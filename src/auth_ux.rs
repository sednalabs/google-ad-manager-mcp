//! Operator-facing authentication helpers.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use serde::Serialize;
use tokio::process::Command;

use crate::config::{
    AuthCommandArgs, AuthDoctorArgs, AuthLoginArgs, AuthStatusCliArgs, AuthSubcommand,
    GCLOUD_ADC_REQUIRED_SCOPE, Settings, adc_credentials_path,
};
use crate::contract::redact_secret_text;
use crate::{AdManagerClient, AuthSource, MANAGE_SCOPE};

pub async fn run_auth_command(settings: &Settings, command: &AuthSubcommand) -> Result<()> {
    match command {
        AuthSubcommand::Login(args) => run_login(settings, args).await,
        AuthSubcommand::Command(args) => print_login_command(settings, args),
        AuthSubcommand::Status(args) => run_status(settings, args).await,
        AuthSubcommand::Doctor(args) => run_doctor(settings, args).await,
    }
}

pub(crate) fn gcloud_adc_login_command(
    scope: &str,
    client_id_file: Option<&Path>,
    headless: bool,
) -> Vec<String> {
    let mut command = vec![
        "gcloud".to_string(),
        "auth".to_string(),
        "application-default".to_string(),
        "login".to_string(),
        format!("--scopes={}", adc_login_scopes(scope).join(",")),
    ];
    if headless {
        command.push("--no-launch-browser".to_string());
    }
    if let Some(path) = client_id_file {
        command.push(format!("--client-id-file={}", path.display()));
    }
    command
}

pub(crate) fn shell_join(parts: &[String]) -> String {
    parts
        .iter()
        .map(|part| {
            if !part.is_empty()
                && part
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || "-_=:/.,+".contains(ch))
            {
                part.clone()
            } else {
                format!("'{}'", part.replace('\'', "'\"'\"'"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

async fn run_login(settings: &Settings, args: &AuthLoginArgs) -> Result<()> {
    let scope = selected_login_scope(settings, args.manage_scope);
    let command = gcloud_adc_login_command(&scope, args.client_id_file.as_deref(), args.headless);
    println!("Starting Google Ad Manager login using Application Default Credentials.");
    println!("Scope: {scope}");
    println!("Command: {}", shell_join(&command));
    println!(
        "Tip: ADC login includes the required cloud-platform scope because gcloud requires it for local ADC user credentials."
    );
    println!(
        "Tip: use --quota-project PROJECT_ID so the server can send x-goog-user-project for the project where the Ad Manager API is enabled."
    );
    if args.headless {
        println!(
            "Headless mode requested; follow the URL and paste the browser result if gcloud asks."
        );
    }

    if args.dry_run {
        return Ok(());
    }

    let status = Command::new(&command[0])
        .args(&command[1..])
        .status()
        .await
        .context("failed to run gcloud ADC login")?;
    if !status.success() {
        return Err(anyhow!("gcloud login failed with status {status}"));
    }

    let quota_project = args
        .quota_project
        .clone()
        .or_else(|| settings.quota_project.clone());
    if let Some(quota_project) = quota_project {
        let set_quota_command = gcloud_set_quota_project_command(&quota_project);
        println!(
            "Setting ADC quota project: {}",
            shell_join(&set_quota_command)
        );
        let status = Command::new(&set_quota_command[0])
            .args(&set_quota_command[1..])
            .status()
            .await
            .context("failed to run gcloud ADC quota-project command")?;
        if !status.success() {
            return Err(anyhow!(
                "gcloud set-quota-project failed with status {status}"
            ));
        }
    }

    println!("Google login completed.");
    let mut verify_settings = settings.clone();
    verify_settings.scope = scope;
    let report = build_report(&verify_settings, !args.no_verify).await;
    print_human_report(&report);
    if !args.no_verify && report.ready == "no" {
        return Err(anyhow!(
            "login completed, but Ad Manager token verification did not pass"
        ));
    }
    Ok(())
}

fn print_login_command(settings: &Settings, args: &AuthCommandArgs) -> Result<()> {
    let scope = selected_login_scope(settings, args.manage_scope);
    let command = gcloud_adc_login_command(&scope, args.client_id_file.as_deref(), args.headless);
    println!("{}", shell_join(&command));
    if let Some(project) = settings.quota_project.as_deref() {
        println!("{}", shell_join(&gcloud_set_quota_project_command(project)));
    }
    Ok(())
}

fn selected_login_scope(settings: &Settings, manage_scope: bool) -> String {
    if manage_scope {
        MANAGE_SCOPE.to_string()
    } else {
        settings.scope.clone()
    }
}

async fn run_status(settings: &Settings, args: &AuthStatusCliArgs) -> Result<()> {
    let report = build_report(settings, args.verify_token).await;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_human_report(&report);
    }
    Ok(())
}

async fn run_doctor(settings: &Settings, args: &AuthDoctorArgs) -> Result<()> {
    let report = build_report(settings, args.verify_token).await;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_human_report(&report);
    }
    Ok(())
}

async fn build_report(settings: &Settings, verify: bool) -> AuthReport {
    let client = AdManagerClient::from_settings(settings);
    let adc_file = adc_credentials_path().map(|path| AdcFileStatus { path: path.clone() });
    let quota_project = effective_quota_project(settings);
    let verification = if verify {
        match client.list_networks(Some(1), None).await {
            Ok(payload) => VerificationReport {
                checked: true,
                ok: Some(true),
                sample_network_count: payload
                    .get("networks")
                    .and_then(|value| value.as_array())
                    .map(Vec::len),
                error: None,
                hint: None,
            },
            Err(err) => VerificationReport {
                checked: true,
                ok: Some(false),
                sample_network_count: None,
                error: Some(redact_secret_text(&err.to_string())),
                hint: Some(err.hint().to_string()),
            },
        }
    } else {
        VerificationReport {
            checked: false,
            ok: None,
            sample_network_count: None,
            error: None,
            hint: None,
        }
    };

    let ready = match verification.ok {
        Some(true) => "yes",
        Some(false) => "no",
        None => "not_verified",
    }
    .to_string();
    let env = EnvStatus {
        google_application_credentials: std::env::var_os("GOOGLE_APPLICATION_CREDENTIALS")
            .is_some(),
        service_account_path: settings.service_account_json_path.is_some(),
        service_account_json: settings.service_account_json.is_some(),
        quota_project: settings.quota_project.is_some(),
    };
    let credential_material_detected = env.google_application_credentials
        || env.service_account_path
        || env.service_account_json
        || verification.ok == Some(true);
    let next_steps = next_steps(settings, &quota_project, &verification);

    AuthReport {
        server: "google-ad-manager-mcp",
        scope: settings.scope.clone(),
        credential_source: client.auth_source(),
        config_valid: true,
        credential_material_detected,
        quota_project,
        gcloud: gcloud_version().await,
        adc_file,
        env,
        verification,
        ready,
        next_steps,
    }
}

fn effective_quota_project(settings: &Settings) -> QuotaProjectStatus {
    if let Some(project) = settings.quota_project.as_deref() {
        return QuotaProjectStatus {
            configured: true,
            value: Some(project.to_string()),
            source: Some("GOOGLE_AD_MANAGER_MCP_QUOTA_PROJECT_or_cli".to_string()),
        };
    }
    QuotaProjectStatus {
        configured: false,
        value: None,
        source: None,
    }
}

async fn gcloud_version() -> Option<String> {
    let output = Command::new("gcloud")
        .arg("--version")
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .and_then(|stdout| stdout.lines().next().map(str::trim).map(str::to_string))
}

fn next_steps(
    settings: &Settings,
    quota_project: &QuotaProjectStatus,
    verification: &VerificationReport,
) -> Vec<String> {
    let mut steps = Vec::new();
    if !verification.checked {
        steps.push(
            "Run `google-ad-manager-mcp auth status --verify-token` when you are ready to prove access."
                .to_string(),
        );
    }
    if !quota_project.configured {
        steps.push(
            "Set `GOOGLE_AD_MANAGER_MCP_QUOTA_PROJECT=PROJECT_ID` in the MCP server environment; `auth login --quota-project PROJECT_ID` also sets the ADC quota project for Google tooling."
                .to_string(),
        );
    }
    if verification.ok == Some(false) {
        let error = verification.error.as_deref().unwrap_or_default();
        if mentions_quota_project(error) {
            steps.push(
                "Set a quota project for ADC and enable the Google Ad Manager API on that project."
                    .to_string(),
            );
        }
        if mentions_scope(error) {
            steps.push(format!(
                "Re-run login with the configured scope: `google-ad-manager-mcp --scope {} auth login --quota-project PROJECT_ID`.",
                settings.scope
            ));
        }
        steps.push(
            "Confirm the Google account or service account has access to the target Ad Manager network."
                .to_string(),
        );
    }
    if steps.is_empty() {
        steps.push(
            "Restart stdio MCP clients that keep a long-lived server child process after changing credentials."
                .to_string(),
        );
    }
    steps
}

fn print_human_report(report: &AuthReport) {
    println!("Google Ad Manager MCP auth");
    println!("Scope: {}", report.scope);
    println!("Credential source: {}", report.credential_source.as_str());
    println!("Config valid: {}", yes_no(report.config_valid));
    println!(
        "Credential material detected: {}",
        yes_no(report.credential_material_detected)
    );
    match (&report.quota_project.value, &report.quota_project.source) {
        (Some(project), Some(source)) => println!("Quota project: {project} ({source})"),
        _ => println!("Quota project: not configured"),
    }
    match &report.gcloud {
        Some(version) => println!("gcloud: {version}"),
        None => println!("gcloud: not available"),
    }
    match &report.adc_file {
        Some(file) => println!("ADC file: conventional path ({})", file.path.display()),
        None => println!("ADC file: unknown"),
    }
    println!(
        "Env credentials: GOOGLE_APPLICATION_CREDENTIALS={}, service-account-path={}, service-account-json={}, quota-project={}",
        yes_no(report.env.google_application_credentials),
        yes_no(report.env.service_account_path),
        yes_no(report.env.service_account_json),
        yes_no(report.env.quota_project),
    );
    if report.verification.checked {
        if report.verification.ok == Some(true) {
            println!(
                "Verification: ok (sample_network_count={})",
                report.verification.sample_network_count.unwrap_or(0)
            );
        } else {
            println!("Verification: failed");
            if let Some(error) = &report.verification.error {
                println!("Error: {error}");
            }
            if let Some(hint) = &report.verification.hint {
                println!("Hint: {hint}");
            }
        }
    } else {
        println!("Verification: not checked");
    }
    println!("Ready: {}", report.ready);
    println!("Next steps:");
    for step in &report.next_steps {
        println!("- {step}");
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn gcloud_set_quota_project_command(project: &str) -> Vec<String> {
    vec![
        "gcloud".to_string(),
        "auth".to_string(),
        "application-default".to_string(),
        "set-quota-project".to_string(),
        project.to_string(),
    ]
}

fn adc_login_scopes(scope: &str) -> Vec<String> {
    let mut scopes = vec![GCLOUD_ADC_REQUIRED_SCOPE.to_string()];
    for scope in scope
        .split([',', ' ', '\n', '\t'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if !scopes.iter().any(|existing| existing == scope) {
            scopes.push(scope.to_string());
        }
    }
    scopes
}

fn mentions_quota_project(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("quota project")
        || lower.contains("x-goog-user-project")
        || lower.contains("service_disabled")
        || lower.contains("api has not been used")
}

fn mentions_scope(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("insufficient authentication scopes")
        || lower.contains("insufficientpermission")
        || lower.contains("forbidden")
}

#[derive(Debug, Serialize)]
struct AuthReport {
    server: &'static str,
    scope: String,
    credential_source: AuthSource,
    config_valid: bool,
    credential_material_detected: bool,
    quota_project: QuotaProjectStatus,
    gcloud: Option<String>,
    adc_file: Option<AdcFileStatus>,
    env: EnvStatus,
    verification: VerificationReport,
    ready: String,
    next_steps: Vec<String>,
}

#[derive(Debug, Serialize)]
struct QuotaProjectStatus {
    configured: bool,
    value: Option<String>,
    source: Option<String>,
}

#[derive(Debug, Serialize)]
struct AdcFileStatus {
    path: PathBuf,
}

#[derive(Debug, Serialize)]
struct EnvStatus {
    google_application_credentials: bool,
    service_account_path: bool,
    service_account_json: bool,
    quota_project: bool,
}

#[derive(Debug, Serialize)]
struct VerificationReport {
    checked: bool,
    ok: Option<bool>,
    sample_network_count: Option<usize>,
    error: Option<String>,
    hint: Option<String>,
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{gcloud_adc_login_command, shell_join};

    #[test]
    fn adc_login_command_includes_cloud_platform_and_ad_manager_scope() {
        let command = gcloud_adc_login_command(
            "https://www.googleapis.com/auth/admanager.readonly",
            Some(Path::new("/tmp/client id.json")),
            true,
        );
        let rendered = shell_join(&command);
        assert!(rendered.contains("application-default login"));
        assert!(rendered.contains("cloud-platform"));
        assert!(rendered.contains("admanager.readonly"));
        assert!(rendered.contains("--no-launch-browser"));
        assert!(rendered.contains("--client-id-file="));
        assert!(rendered.contains("/tmp/client id.json"));
    }

    #[test]
    fn shell_join_quotes_empty_args() {
        let command = vec!["a".to_string(), String::new(), "b".to_string()];
        assert_eq!(shell_join(&command), "a '' b");
    }
}
