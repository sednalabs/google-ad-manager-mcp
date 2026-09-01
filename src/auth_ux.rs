//! Operator-facing authentication helpers.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use gcp_auth::{CustomServiceAccount, Error as GcpAuthError};
use mcp_toolkit_auth::provider_auth::{
    GoogleProviderAuthConfig, GoogleProviderAuthFailureKind, classify_google_provider_auth_error,
    format_provider_auth_command, google_adc_quota_project_command,
};
use mcp_toolkit_auth::upstream_oauth::{
    UpstreamOAuthError, google_authorized_user_adc_metadata_from_file,
};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::process::Command;

use crate::client::auth_source_from_settings;
use crate::config::{
    AuthCommandArgs, AuthDoctorArgs, AuthLoginArgs, AuthStatusCliArgs, AuthSubcommand, Settings,
    conventional_adc_credentials_path, selected_adc_file, server_adc_credentials_path,
    server_cloudsdk_config_dir,
};
use crate::contract::redact_secret_text;
use crate::{AdManagerClient, AdManagerError, AuthSource, MANAGE_SCOPE};

const AD_MANAGER_API_NAME: &str = "Google Ad Manager API";
const AD_MANAGER_API_SERVICE: &str = "admanager.googleapis.com";

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
    if let Some(path) = client_id_file {
        ad_manager_provider_auth_config(scope)
            .adc_login_command_with_client_id_file(headless, &path.display().to_string())
    } else {
        ad_manager_provider_auth_config(scope).adc_login_command(headless)
    }
}

pub(crate) fn shell_join(parts: &[String]) -> String {
    format_provider_auth_command(parts)
}

async fn run_login(settings: &Settings, args: &AuthLoginArgs) -> Result<()> {
    let scope = selected_login_scope(settings, args.manage_scope);
    let shared_adc = auth_command_shared_adc(settings, args.shared_adc);
    let command = gcloud_adc_login_command(&scope, args.client_id_file.as_deref(), args.headless);
    let cloudsdk_config = require_login_cloudsdk_config(shared_adc)?;
    println!("Starting Google Ad Manager login using Application Default Credentials.");
    println!("Scope: {scope}");
    println!(
        "Credential file: {}",
        adc_login_target_description(shared_adc)
    );
    println!(
        "Command: {}",
        shell_join_with_cloudsdk_config(&command, cloudsdk_config.as_deref())
    );
    println!(
        "Tip: ADC login includes the required cloud-platform scope because gcloud requires it for local ADC user credentials."
    );
    if !shared_adc {
        println!(
            "Tip: this login uses a Google Ad Manager-specific ADC file so other Google MCPs keep their own tokens and scopes."
        );
    }
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

    if let Some(dir) = cloudsdk_config.as_deref() {
        fs::create_dir_all(dir).context("failed to create server-specific gcloud config dir")?;
    }

    let mut login = Command::new(&command[0]);
    login.args(&command[1..]);
    if let Some(dir) = cloudsdk_config.as_deref() {
        login.env("CLOUDSDK_CONFIG", dir);
    }
    let status = login
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
            shell_join_with_cloudsdk_config(&set_quota_command, cloudsdk_config.as_deref())
        );
        let mut quota = Command::new(&set_quota_command[0]);
        quota.args(&set_quota_command[1..]);
        if let Some(dir) = cloudsdk_config.as_deref() {
            quota.env("CLOUDSDK_CONFIG", dir);
        }
        let status = quota
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
    verify_settings.shared_adc = shared_adc;
    let report = build_report(&verify_settings, false, !args.no_verify).await;
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
    let cloudsdk_config =
        require_login_cloudsdk_config(auth_command_shared_adc(settings, args.shared_adc))?;
    println!(
        "{}",
        shell_join_with_cloudsdk_config(&command, cloudsdk_config.as_deref())
    );
    if let Some(project) = settings.quota_project.as_deref() {
        println!(
            "{}",
            shell_join_with_cloudsdk_config(
                &gcloud_set_quota_project_command(project),
                cloudsdk_config.as_deref(),
            )
        );
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

fn auth_command_shared_adc(settings: &Settings, shared_adc_flag: bool) -> bool {
    shared_adc_flag || settings.shared_adc
}

async fn run_status(settings: &Settings, args: &AuthStatusCliArgs) -> Result<()> {
    let report = build_report(settings, args.verify_token, args.verify_access).await;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_human_report(&report);
    }
    Ok(())
}

async fn run_doctor(settings: &Settings, args: &AuthDoctorArgs) -> Result<()> {
    let report = build_report(settings, args.verify_token, args.verify_access).await;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_human_report(&report);
    }
    Ok(())
}

async fn build_report(settings: &Settings, verify_token: bool, verify_access: bool) -> AuthReport {
    let env = EnvStatus {
        google_application_credentials: std::env::var_os("GOOGLE_APPLICATION_CREDENTIALS")
            .is_some(),
        service_account_path: settings.service_account_json_path.is_some(),
        service_account_json: settings.service_account_json.is_some(),
        quota_project: settings.quota_project.is_some(),
        shared_adc: settings.shared_adc,
    };
    let uses_local_user_adc = uses_local_user_adc(&env);
    let credential_status = credential_source_status(settings, uses_local_user_adc);
    let credential_source = auth_source_from_settings(settings);
    let reported_credential_source = reported_auth_source(
        credential_status.config_valid,
        credential_source.as_ref().copied().ok(),
    );
    let quota_project =
        effective_quota_project(settings, credential_status.adc_file.as_ref(), &env);
    let token_check = if verify_token || verify_access {
        match credential_source.as_ref() {
            Ok(_) => {
                let client = AdManagerClient::from_settings(settings);
                match client.verify_token().await {
                    Ok(()) => VerificationReport {
                        checked: true,
                        ok: Some(true),
                        sample_network_count: None,
                        error: None,
                        hint: None,
                        reason: None,
                    },
                    Err(err) => verification_failure(&err),
                }
            }
            Err(err) => verification_failure(err),
        }
    } else {
        VerificationReport::skipped("not_requested")
    };
    let access_check = if !verify_access {
        VerificationReport::skipped("not_requested")
    } else if token_check.ok != Some(true) {
        VerificationReport::skipped("token_check_failed")
    } else {
        match credential_source.as_ref() {
            Ok(_) => {
                let client = AdManagerClient::from_settings(settings);
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
                        reason: None,
                    },
                    Err(err) => verification_failure(&err),
                }
            }
            Err(err) => verification_failure(err),
        }
    };
    let verification = if verify_access {
        access_check.clone()
    } else {
        token_check.clone()
    };

    let config_issue = credential_status.config_issue.clone().or_else(|| {
        credential_source
            .as_ref()
            .err()
            .map(|err| redact_secret_text(&err.to_string()))
    });
    let config_valid = credential_status.config_valid && credential_source.is_ok();
    let ready = readiness(config_valid, &token_check, &access_check, &verification);
    let credential_material_detected =
        credential_status.credential_material_detected || verification.ok == Some(true);
    let next_steps = next_steps(
        settings,
        &quota_project,
        &token_check,
        &access_check,
        &credential_status,
    );

    AuthReport {
        server: "google-ad-manager-mcp",
        scope: settings.scope.clone(),
        credential_source: reported_credential_source,
        config_valid,
        config_issue,
        credential_material_detected,
        quota_project,
        gcloud: gcloud_version().await,
        adc_file: credential_status.adc_file,
        env,
        token_check,
        access_check,
        verification,
        ready,
        next_steps,
    }
}

fn readiness(
    config_valid: bool,
    token_check: &VerificationReport,
    access_check: &VerificationReport,
    verification: &VerificationReport,
) -> String {
    if !config_valid || token_check.ok == Some(false) || access_check.ok == Some(false) {
        "no".to_string()
    } else {
        match verification.ok {
            Some(true) => "yes",
            Some(false) => "no",
            None => "not_verified",
        }
        .to_string()
    }
}

fn effective_quota_project(
    settings: &Settings,
    adc_file: Option<&AdcFileStatus>,
    env: &EnvStatus,
) -> QuotaProjectStatus {
    if let Some(project) = settings.quota_project.as_deref() {
        return QuotaProjectStatus {
            configured: true,
            value: Some(project.to_string()),
            source: Some("GOOGLE_AD_MANAGER_MCP_QUOTA_PROJECT_or_cli".to_string()),
        };
    }
    if uses_local_user_adc(env)
        && let Some(project) = adc_file.and_then(|status| status.quota_project_id.as_ref())
    {
        return QuotaProjectStatus {
            configured: true,
            value: Some(project.to_string()),
            source: Some("selected_adc_file".to_string()),
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
    token_check: &VerificationReport,
    access_check: &VerificationReport,
    credential_status: &CredentialSourceStatus,
) -> Vec<String> {
    let mut steps = Vec::new();
    if let Some(step) = &credential_status.repair_step {
        steps.push(step.clone());
    }
    if !token_check.checked {
        let step = if credential_status.config_valid {
            "Run `google-ad-manager-mcp auth status --verify-token` when you are ready to prove access."
        } else {
            "After fixing the credential configuration, run `google-ad-manager-mcp auth status --verify-token` to prove access."
        };
        steps.push(step.to_string());
    }
    if token_check.checked && !access_check.checked {
        steps.push(
            "Run `google-ad-manager-mcp auth status --verify-access` when you are ready to prove Ad Manager network access."
                .to_string(),
        );
    }
    if !quota_project.configured {
        steps.push(
            "Set `GOOGLE_AD_MANAGER_MCP_QUOTA_PROJECT=PROJECT_ID` in the MCP server environment; `auth login --quota-project PROJECT_ID` also sets the ADC quota project for Google tooling."
                .to_string(),
        );
    }
    if token_check.ok == Some(false) || access_check.ok == Some(false) {
        let verification = if access_check.ok == Some(false) {
            access_check
        } else {
            token_check
        };
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
    if let Some(issue) = &report.config_issue {
        println!("Credential config issue: {issue}");
    }
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
        Some(file) => {
            println!(
                "ADC file: {} ({}, {})",
                if file.present { "present" } else { "missing" },
                file.kind,
                file.path.display()
            );
            println!("ADC selection: {}", file.selection_source);
            if let Some(usable) = file.usable {
                println!("ADC file usable: {}", yes_no(usable));
            }
            if let Some(error) = &file.error {
                println!("ADC file issue: {error}");
            }
        }
        None => println!("ADC file: not selected for current credential source"),
    }
    println!(
        "Env credentials: GOOGLE_APPLICATION_CREDENTIALS={}, service-account-path={}, service-account-json={}, quota-project={}, shared-adc={}",
        yes_no(report.env.google_application_credentials),
        yes_no(report.env.service_account_path),
        yes_no(report.env.service_account_json),
        yes_no(report.env.quota_project),
        yes_no(report.env.shared_adc),
    );
    print_verification("Token check", &report.token_check);
    print_verification("Access check", &report.access_check);
    println!("Ready: {}", report.ready);
    println!("Next steps:");
    for step in &report.next_steps {
        println!("- {step}");
    }
}

fn print_verification(label: &str, verification: &VerificationReport) {
    if !verification.checked {
        println!(
            "{label}: skipped ({})",
            verification.reason.as_deref().unwrap_or("not_requested")
        );
    } else if verification.ok == Some(true) {
        if let Some(count) = verification.sample_network_count {
            println!("{label}: ok (sample_network_count={count})");
        } else {
            println!("{label}: ok");
        }
    } else {
        println!("{label}: failed");
        if let Some(error) = &verification.error {
            println!("Error: {error}");
        }
        if let Some(hint) = &verification.hint {
            println!("Hint: {hint}");
        }
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn gcloud_set_quota_project_command(project: &str) -> Vec<String> {
    google_adc_quota_project_command(project)
}

fn login_cloudsdk_config_dir(shared_adc: bool) -> Option<PathBuf> {
    if shared_adc {
        None
    } else {
        server_cloudsdk_config_dir()
    }
}

fn require_login_cloudsdk_config(shared_adc: bool) -> Result<Option<PathBuf>> {
    let cloudsdk_config = login_cloudsdk_config_dir(shared_adc);
    if !shared_adc && cloudsdk_config.is_none() {
        return Err(anyhow!(
            "failed to determine the server-specific gcloud config directory; set HOME/XDG_CONFIG_HOME on Unix or APPDATA on Windows, or pass --shared-adc to intentionally use conventional shared ADC"
        ));
    }
    Ok(cloudsdk_config)
}

fn adc_login_target_description(shared_adc: bool) -> String {
    if shared_adc {
        return conventional_adc_credentials_path()
            .map(|path| format!("shared gcloud ADC ({})", path.display()))
            .unwrap_or_else(|| "shared gcloud ADC".to_string());
    }
    server_adc_credentials_path()
        .map(|path| format!("server-specific ADC ({})", path.display()))
        .unwrap_or_else(|| "server-specific ADC".to_string())
}

pub(crate) fn shell_join_with_cloudsdk_config(
    parts: &[String],
    cloudsdk_config: Option<&Path>,
) -> String {
    if let Some(dir) = cloudsdk_config {
        let assignment = format!(
            "CLOUDSDK_CONFIG={}",
            shell_join(&[dir.display().to_string()])
        );
        let command = shell_join(parts);
        if command.is_empty() {
            assignment
        } else {
            format!("{assignment} {command}")
        }
    } else {
        shell_join(parts)
    }
}

fn selected_adc_file_status(settings: &Settings) -> Option<AdcFileStatus> {
    let selected = selected_adc_file(settings.shared_adc)?;
    let path = selected.path.clone();
    match google_authorized_user_adc_metadata_from_file(&path) {
        Ok(Some(metadata)) => {
            let usable = metadata.client_id_present()
                && metadata.client_secret_present()
                && metadata.refresh_token_present();
            let error = if usable {
                None
            } else {
                Some(format!(
                    "missing required authorized-user fields in {} ADC file",
                    selected.source.kind_label()
                ))
            };
            Some(AdcFileStatus {
                selection_source: selected.source.as_str(),
                kind: selected.source.kind_label(),
                path,
                present: true,
                usable: Some(usable),
                quota_project_id: metadata.quota_project_id().map(str::to_string),
                error,
            })
        }
        Ok(None) => Some(AdcFileStatus {
            selection_source: selected.source.as_str(),
            kind: selected.source.kind_label(),
            path,
            present: false,
            usable: None,
            quota_project_id: None,
            error: None,
        }),
        Err(err) => Some(AdcFileStatus {
            selection_source: selected.source.as_str(),
            kind: selected.source.kind_label(),
            present: true,
            path,
            usable: Some(false),
            quota_project_id: None,
            error: Some(redact_secret_text(&err.to_string())),
        }),
    }
}

fn credential_source_status(
    settings: &Settings,
    uses_local_user_adc: bool,
) -> CredentialSourceStatus {
    if uses_local_user_adc {
        let adc_file = selected_adc_file_status(settings);
        if adc_file.is_none() {
            let repair_step = if settings.shared_adc {
                "Set CLOUDSDK_CONFIG, HOME/XDG_CONFIG_HOME, or APPDATA so the conventional shared ADC path can be resolved, or disable shared ADC to use the server-specific default."
            } else {
                "Set HOME/XDG_CONFIG_HOME or APPDATA so the server-specific ADC path can be resolved, or enable GOOGLE_AD_MANAGER_MCP_SHARED_ADC=true to intentionally use conventional shared ADC."
            };
            return CredentialSourceStatus {
                config_valid: false,
                config_issue: Some(
                    "failed to determine the selected ADC path for local authorized-user credentials"
                        .to_string(),
                ),
                credential_material_detected: false,
                repair_step: Some(repair_step.to_string()),
                adc_file: None,
            };
        }
        let config_valid = adc_file
            .as_ref()
            .is_some_and(|file| file.present && file.usable != Some(false));
        let credential_material_detected = adc_file.as_ref().is_some_and(|file| file.present);
        let config_issue = adc_file.as_ref().and_then(|file| {
            file.error.clone().or_else(|| {
                (!file.present).then(|| {
                    format!(
                        "selected {} ADC file is missing at {}",
                        file.kind,
                        file.path.display()
                    )
                })
            })
        });
        let repair_step = adc_file.as_ref().and_then(|file| {
            if !file.present {
                Some(selected_adc_missing_step(file))
            } else if file.usable == Some(false) {
                Some(selected_adc_repair_step(settings, file))
            } else {
                None
            }
        });
        return CredentialSourceStatus {
            config_valid,
            config_issue,
            credential_material_detected,
            repair_step,
            adc_file,
        };
    }

    if let Some(path) = settings.service_account_json_path.as_deref() {
        return service_account_json_path_status(path);
    }
    if let Some(raw_json) = settings.service_account_json.as_deref() {
        return service_account_json_env_status(raw_json);
    }
    if let Some(path) = std::env::var_os("GOOGLE_APPLICATION_CREDENTIALS") {
        return google_application_credentials_status(PathBuf::from(path));
    }
    CredentialSourceStatus {
        config_valid: false,
        config_issue: Some("no credential source was selected".to_string()),
        credential_material_detected: false,
        repair_step: Some(
            "Configure a service-account credential, GOOGLE_APPLICATION_CREDENTIALS, or rerun `google-ad-manager-mcp auth login`."
                .to_string(),
        ),
        adc_file: None,
    }
}

fn service_account_json_path_status(path: &str) -> CredentialSourceStatus {
    match CustomServiceAccount::from_file(path) {
        Ok(_) => CredentialSourceStatus {
            config_valid: true,
            config_issue: None,
            credential_material_detected: true,
            repair_step: None,
            adc_file: None,
        },
        Err(err) => CredentialSourceStatus {
            config_valid: false,
            config_issue: Some(redact_secret_text(&format!(
                "failed to load service account JSON at {path}: {err}"
            ))),
            credential_material_detected: true,
            repair_step: Some(
                "Fix `GOOGLE_AD_MANAGER_MCP_SERVICE_ACCOUNT_JSON_PATH` so it points to a valid service account JSON file, or unset it to use another credential source."
                    .to_string(),
            ),
            adc_file: None,
        },
    }
}

fn service_account_json_env_status(raw_json: &str) -> CredentialSourceStatus {
    match CustomServiceAccount::from_json(raw_json) {
        Ok(_) => CredentialSourceStatus {
            config_valid: true,
            config_issue: None,
            credential_material_detected: true,
            repair_step: None,
            adc_file: None,
        },
        Err(err) => CredentialSourceStatus {
            config_valid: false,
            config_issue: Some(redact_secret_text(&format!(
                "invalid service account JSON in GOOGLE_AD_MANAGER_MCP_SERVICE_ACCOUNT_JSON: {err}"
            ))),
            credential_material_detected: true,
            repair_step: Some(
                "Fix `GOOGLE_AD_MANAGER_MCP_SERVICE_ACCOUNT_JSON`, or unset it to use another credential source."
                    .to_string(),
            ),
            adc_file: None,
        },
    }
}

fn google_application_credentials_status(path: PathBuf) -> CredentialSourceStatus {
    match CustomServiceAccount::from_file(&path) {
        Ok(_) => CredentialSourceStatus {
            config_valid: true,
            config_issue: None,
            credential_material_detected: true,
            repair_step: None,
            adc_file: None,
        },
        Err(service_account_err) => match google_authorized_user_adc_metadata_from_file(&path) {
            Ok(Some(_metadata)) => CredentialSourceStatus {
                config_valid: false,
                config_issue: Some(format!(
                    "GOOGLE_APPLICATION_CREDENTIALS points to an authorized-user ADC file at {}; google-ad-manager-mcp only supports service-account credentials on GOOGLE_APPLICATION_CREDENTIALS",
                    path.display()
                )),
                credential_material_detected: true,
                repair_step: Some(
                    "Unset `GOOGLE_APPLICATION_CREDENTIALS` and use `google-ad-manager-mcp auth login` for user credentials, or point `GOOGLE_APPLICATION_CREDENTIALS` at a valid service-account JSON file."
                        .to_string(),
                ),
                adc_file: None,
            },
            Ok(None) | Err(UpstreamOAuthError::UnsupportedGoogleAdcCredentialType) | Err(_) => {
                CredentialSourceStatus {
                    config_valid: false,
                    config_issue: Some(redact_secret_text(&format!(
                        "failed to load GOOGLE_APPLICATION_CREDENTIALS at {}: {service_account_err}",
                        path.display()
                    ))),
                    credential_material_detected:
                        credential_material_detected_from_gcp_auth_error(&service_account_err),
                    repair_step: Some(
                        "Fix `GOOGLE_APPLICATION_CREDENTIALS` so it points to a readable credentials file, or unset it to use another credential source."
                            .to_string(),
                    ),
                    adc_file: None,
                }
            }
        },
    }
}

fn credential_material_detected_from_gcp_auth_error(err: &GcpAuthError) -> bool {
    !matches!(
        err,
        GcpAuthError::Io(_, source) if source.kind() == ErrorKind::NotFound
    )
}

fn uses_local_user_adc(env: &EnvStatus) -> bool {
    !env.google_application_credentials && !env.service_account_path && !env.service_account_json
}

fn reported_auth_source(config_valid: bool, selected: Option<AuthSource>) -> AuthSource {
    if config_valid {
        selected.unwrap_or(AuthSource::Unavailable)
    } else {
        AuthSource::Unavailable
    }
}

pub(crate) fn validated_auth_source(settings: &Settings) -> AuthSource {
    let env = EnvStatus {
        google_application_credentials: std::env::var_os("GOOGLE_APPLICATION_CREDENTIALS")
            .is_some(),
        service_account_path: settings.service_account_json_path.is_some(),
        service_account_json: settings.service_account_json.is_some(),
        quota_project: settings.quota_project.is_some(),
        shared_adc: settings.shared_adc,
    };
    let status = credential_source_status(settings, uses_local_user_adc(&env));
    reported_auth_source(
        status.config_valid,
        auth_source_from_settings(settings).ok(),
    )
}

/// Returns the secret-safe, non-network authentication diagnostics shared by
/// the CLI and MCP status surfaces. In particular, this reports which ADC
/// file was selected, whether it is present/usable, and its quota-project
/// metadata without exposing credential material.
pub(crate) fn mcp_auth_diagnostics(settings: &Settings) -> Value {
    let env = EnvStatus {
        google_application_credentials: std::env::var_os("GOOGLE_APPLICATION_CREDENTIALS")
            .is_some(),
        service_account_path: settings.service_account_json_path.is_some(),
        service_account_json: settings.service_account_json.is_some(),
        quota_project: settings.quota_project.is_some(),
        shared_adc: settings.shared_adc,
    };
    let status = credential_source_status(settings, uses_local_user_adc(&env));
    let auth_source = reported_auth_source(
        status.config_valid,
        auth_source_from_settings(settings).ok(),
    );
    let quota_project = effective_quota_project(settings, status.adc_file.as_ref(), &env);
    json!({
        "auth_source": auth_source.as_str(),
        "config_valid": status.config_valid,
        "config_issue": status.config_issue,
        "credential_material_detected": status.credential_material_detected,
        "adc_file": status.adc_file,
        "quota_project": quota_project,
    })
}

fn selected_adc_missing_step(adc_file: &AdcFileStatus) -> String {
    match adc_file.selection_source {
        "server_specific_default" => format!(
            "Run `google-ad-manager-mcp auth login --headless --quota-project PROJECT_ID` to create the server-specific ADC file at {}, or set GOOGLE_AD_MANAGER_MCP_SHARED_ADC=true to intentionally use conventional shared ADC.",
            adc_file.path.display()
        ),
        "shared_explicit" => format!(
            "Run `google-ad-manager-mcp auth login --shared-adc --headless --quota-project PROJECT_ID` to create the shared ADC file at {}, or clear GOOGLE_AD_MANAGER_MCP_SHARED_ADC to return to the server-specific default.",
            adc_file.path.display()
        ),
        _ => format!(
            "Create the selected ADC file at {} before retrying auth.",
            adc_file.path.display()
        ),
    }
}

fn selected_adc_repair_step(settings: &Settings, adc_file: &AdcFileStatus) -> String {
    if settings.shared_adc {
        format!(
            "Repair or replace the shared ADC file at {}, rerun `google-ad-manager-mcp auth login --shared-adc`, or clear GOOGLE_AD_MANAGER_MCP_SHARED_ADC to return to the server-specific default.",
            adc_file.path.display()
        )
    } else {
        format!(
            "Repair or replace the server-specific ADC file at {}, or rerun `google-ad-manager-mcp auth login --headless --quota-project PROJECT_ID`.",
            adc_file.path.display()
        )
    }
}

fn mentions_quota_project(error: &str) -> bool {
    let diagnostic = classify_google_provider_auth_error(
        403,
        error,
        &ad_manager_provider_auth_config(MANAGE_SCOPE),
    );
    if matches!(
        diagnostic.kind,
        GoogleProviderAuthFailureKind::MissingQuotaProject
            | GoogleProviderAuthFailureKind::ApiDisabled
    ) {
        return true;
    }
    let lower = error.to_ascii_lowercase();
    lower.contains("quota project")
        || lower.contains("x-goog-user-project")
        || lower.contains("service_disabled")
        || lower.contains("api has not been used")
}

fn mentions_scope(error: &str) -> bool {
    let diagnostic = classify_google_provider_auth_error(
        403,
        error,
        &ad_manager_provider_auth_config(MANAGE_SCOPE),
    );
    if diagnostic.kind == GoogleProviderAuthFailureKind::MissingScope {
        return true;
    }
    let lower = error.to_ascii_lowercase();
    lower.contains("insufficient authentication scopes")
        || lower.contains("insufficientpermission")
        || lower.contains("forbidden")
}

fn ad_manager_provider_auth_config(scope: &str) -> GoogleProviderAuthConfig {
    GoogleProviderAuthConfig::new(AD_MANAGER_API_NAME, split_scopes(scope))
        .with_api_service_name(AD_MANAGER_API_SERVICE)
}

fn split_scopes(scope: &str) -> Vec<String> {
    scope
        .split([',', ' ', '\n', '\t'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

#[derive(Debug, Serialize)]
struct AuthReport {
    server: &'static str,
    scope: String,
    credential_source: AuthSource,
    config_valid: bool,
    config_issue: Option<String>,
    credential_material_detected: bool,
    quota_project: QuotaProjectStatus,
    gcloud: Option<String>,
    adc_file: Option<AdcFileStatus>,
    env: EnvStatus,
    token_check: VerificationReport,
    access_check: VerificationReport,
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
    selection_source: &'static str,
    kind: &'static str,
    path: PathBuf,
    present: bool,
    usable: Option<bool>,
    quota_project_id: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct EnvStatus {
    google_application_credentials: bool,
    service_account_path: bool,
    service_account_json: bool,
    quota_project: bool,
    shared_adc: bool,
}

#[derive(Debug)]
struct CredentialSourceStatus {
    config_valid: bool,
    config_issue: Option<String>,
    credential_material_detected: bool,
    repair_step: Option<String>,
    adc_file: Option<AdcFileStatus>,
}

fn verification_failure(err: &AdManagerError) -> VerificationReport {
    VerificationReport {
        checked: true,
        ok: Some(false),
        sample_network_count: None,
        error: Some(redact_secret_text(&err.to_string())),
        hint: Some(err.hint().to_string()),
        reason: None,
    }
}

#[derive(Debug, Clone, Serialize)]
struct VerificationReport {
    checked: bool,
    ok: Option<bool>,
    sample_network_count: Option<usize>,
    error: Option<String>,
    hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

impl VerificationReport {
    fn skipped(reason: &'static str) -> Self {
        Self {
            checked: false,
            ok: None,
            sample_network_count: None,
            error: None,
            hint: None,
            reason: Some(reason.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::AdManagerError;
    use crate::config::Settings;

    use super::{
        auth_command_shared_adc, gcloud_adc_login_command, google_application_credentials_status,
        shell_join, shell_join_with_cloudsdk_config, verification_failure,
    };

    // Test-only generated 2048-bit key with no external identity or authority.
    const TEST_SERVICE_ACCOUNT_PRIVATE_KEY: &str = r#"-----BEGIN PRIVATE KEY-----
MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQCipY/z9r4eloUL
51qos5sGp8StxDLW8wo015Zd9QZOH9qJfJztgIDJpAQuf3/9jw1Z6JP5E5nOyYGa
9mYMyiND/YP6PajkHSNy0sJ/0Fsymf8M8KVBejOILzgtlswfLxZeaTjLVtnXMfSC
QeGfKdcFdKHG7kR/ljip22o5aBavm9jonEHU1gWfF2CDiIY/YFcl8SFwMkgSxy0L
y7dPthhvOgHZZE+KQFOYoqkC26I24GC+TWU7NOcQ3HcghYjVsmqPUaiS1KMSlh90
JDIH3vgcHvNQepn9VMXyWY44XPmbZqy3JYRlsiFMIH1utPtlkkK7DvrdIYfhSoKv
itq7uurBAgMBAAECggEAFCY8ojWkMfflvabItXOitf1cwUY4Iibz0b4Pk85CHLWX
hkbYzheIXPKjzfrfqVLqjYPhqQ7DlDmkg8UYuWblXYvvqLWw0anGdXgkvl7anXc0
gK7jWixAbBOlewhee1KDC+kvLwmwbRd0OhrdT7GIQNXFIPbtp3y9wlU7YKdDgDe0
mUNxfbLvrtRD/0D7vp5jI+DHB5X1BdrFurygoduOHiIOQB8taVevmWf3BewzU/VF
6yPSGF5I4SmIaQd0ZIqd3YCmMCwrbY06wb2w6hvJLGnpAFRDClKjFzVyCzT5PiCA
B9zQs+inwFX/+u3cmyc5LLVprglDL5gkTe7OiP0oQQKBgQDjb7EGF1R/pZfOmOUo
yiRal+b1DNRRBlND6PqqBz4tWqr7xXtjie+ACme9pNwX6t3EYuWsAw8DDU0OLFDc
LvyKEg32lKuNJLaywqJIxQM3twk8yP1SV1nCgNdh3Puktn0vzkgR3MSVqgSkpbB2
L3SItWttUVwbx3cuyZ4/Uzo6qQKBgQC3EtH8GzPZunAgEscW95xaUdFzuzZ4FE95
V7bemhih6CBGXSSPycusQ9zBHoTwnCSa5MMn6ys+PdECBOPQ0T7Vq6FaHyFvrNXM
Q8P6vFM+u7KhLNxtwDl2mXD7HdTIrDo72Yu7mdE1tiuaMs8fHRX9wLGSa0vM37wv
6z/dmSIWWQKBgQDb9g64PGINngKG3dprq6yjLVxCTakdv8dR24ZqYNzikljhbSob
p7DJHccdY881FoJqx9cmmEKxifCnL3b4rDyz8Cgu/bQ4qnRDyPeY92lYPh6h+iT9
uNtnwKIN1OJPd+r1DEUpeWFq+ebJsjFK7DSBbyw5qsExYKVEy9vPlNexGQKBgQCr
8zZdd2NdBjrYNSrfzJQDVUPIUrfXUyROUW+GZu/p6m+eB1AW6a+uTlMi5DpzEAVl
oqYWcVC9dixAnD0p3c8Ju9miHwk1rf1ljOSfNZFuo7ckoVEsmFagqYAvrJY2IWXU
3wDapJ+WtlL/0uctTxFftERUxQh+FkrYKzpiNbmJiQKBgDVTcX+FMk4KpfMRJRDa
lSiQ6gMViABsH1fSdfZOWb9A2Ng54e7W+s/YcWO4wxxJTkt/3OJ8r+6cClChJiGl
kvD+Ch5Kug6TGdYBczUNoK0EVKiOA8fZ+4a+ny9AeDzcV9XetVk14M2FPKPkOySR
sbhtpi32ZJCvwpBEP6g7HaOR
-----END PRIVATE KEY-----"#;

    fn test_service_account_json() -> String {
        serde_json::json!({
            "type": "service_account",
            "project_id": "test-project",
            "private_key_id": "test-key-id",
            "private_key": TEST_SERVICE_ACCOUNT_PRIVATE_KEY,
            "client_email": "test-service-account@example.iam.gserviceaccount.com",
            "client_id": "123456789012345678901",
            "auth_uri": "https://accounts.google.com/o/oauth2/auth",
            "token_uri": "https://oauth2.googleapis.com/token",
            "auth_provider_x509_cert_url": "https://www.googleapis.com/oauth2/v1/certs",
            "client_x509_cert_url": "https://www.googleapis.com/robot/v1/metadata/x509/test-service-account%40example.iam.gserviceaccount.com"
        })
        .to_string()
    }

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
        assert!(rendered.contains("--no-browser"));
        assert!(rendered.contains("--client-id-file"));
        assert!(rendered.contains("/tmp/client id.json"));
    }

    #[test]
    fn shell_join_quotes_empty_args() {
        let command = vec!["a".to_string(), String::new(), "b".to_string()];
        assert_eq!(shell_join(&command), "a '' b");
    }

    #[test]
    fn shell_join_with_cloudsdk_config_prefixes_login_environment() {
        let command = gcloud_adc_login_command(
            "https://www.googleapis.com/auth/admanager.readonly",
            None,
            true,
        );
        let rendered = shell_join_with_cloudsdk_config(&command, Some(Path::new("/tmp/gam adc")));
        assert!(rendered.starts_with("CLOUDSDK_CONFIG='/tmp/gam adc' gcloud auth"));
        assert!(rendered.contains("admanager.readonly"));
    }

    #[test]
    fn auth_command_shared_adc_follows_runtime_selection() {
        assert!(!auth_command_shared_adc(&Settings::default(), false));
        assert!(auth_command_shared_adc(&Settings::default(), true));
    }

    #[test]
    fn google_application_credentials_rejects_authorized_user_adc_files() {
        let path = unique_test_file("google-application-credentials-authorized-user", "json");
        fs::create_dir_all(path.parent().expect("test file parent")).expect("create test dir");
        fs::write(
            &path,
            r#"{
  "type": "authorized_user",
  "client_id": "client-id",
  "client_secret": "client-secret",
  "refresh_token": "refresh-token"
}"#,
        )
        .expect("write authorized-user adc");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("chmod");
        }
        let status = google_application_credentials_status(path.clone());
        assert!(!status.config_valid);
        assert!(status.credential_material_detected);
        assert!(
            status
                .config_issue
                .as_deref()
                .is_some_and(|issue| issue.contains("authorized-user ADC"))
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn google_application_credentials_accepts_service_account_json_file() {
        let path = unique_test_file("google-application-credentials-service-account", "json");
        fs::create_dir_all(path.parent().expect("test file parent")).expect("create test dir");
        fs::write(&path, test_service_account_json()).expect("write service-account json");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("chmod");
        }
        let status = google_application_credentials_status(path.clone());
        assert!(status.config_valid, "{:#?}", status.config_issue);
        assert!(status.config_issue.is_none());
        assert!(status.credential_material_detected);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn google_application_credentials_missing_file_does_not_claim_material_detected() {
        let path = unique_test_file("google-application-credentials-missing", "json");
        let status = google_application_credentials_status(path.clone());
        assert!(!status.config_valid);
        assert!(!status.credential_material_detected);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn unavailable_source_is_reported_truthfully() {
        assert_eq!(
            super::reported_auth_source(
                false,
                Some(crate::AuthSource::GoogleAuthorizedUserAdcFile)
            ),
            crate::AuthSource::Unavailable
        );
        assert_eq!(
            serde_json::to_string(&super::reported_auth_source(false, None))
                .expect("serialize unavailable source"),
            r#""unavailable""#
        );
    }

    #[test]
    fn verification_errors_are_redacted_before_display() {
        let report = verification_failure(&AdManagerError::AuthBootstrap(
            "google_access_token=opaque-secret".to_string(),
        ));
        let serialized = serde_json::to_string(&report).expect("serialize verification");
        assert!(serialized.contains("[redacted]"));
        assert!(!serialized.contains("opaque-secret"));
    }

    #[test]
    fn skipped_verification_has_explicit_reason() {
        let report = super::VerificationReport::skipped("token_check_failed");
        let serialized = serde_json::to_string(&report).expect("serialize skipped verification");
        assert!(serialized.contains(r#""checked":false"#));
        assert!(serialized.contains(r#""reason":"token_check_failed"#));
    }

    #[test]
    fn failed_token_with_skipped_access_is_not_ready() {
        let token = verification_failure(&AdManagerError::AuthBootstrap(
            "token acquisition failed".to_string(),
        ));
        let access = VerificationReport::skipped("token_check_failed");
        let ready = readiness(true, &token, &access, &access);
        assert_eq!(ready, "no");
    }

    #[test]
    fn mcp_auth_diagnostics_exposes_safe_configuration_state() {
        let diagnostics = super::mcp_auth_diagnostics(&Settings::default());
        assert!(diagnostics.get("config_valid").is_some());
        assert!(diagnostics.get("credential_material_detected").is_some());
        assert!(diagnostics.get("quota_project").is_some());
    }

    fn unique_test_file(label: &str, extension: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        PathBuf::from("target")
            .join("google-ad-manager-mcp-auth-ux-tests")
            .join(format!("{label}-{suffix}.{extension}"))
    }
}
