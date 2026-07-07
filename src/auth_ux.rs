//! Operator-facing authentication helpers.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use gcp_auth::CustomServiceAccount;
use mcp_toolkit_auth::provider_auth::{
    GoogleProviderAuthConfig, GoogleProviderAuthFailureKind, classify_google_provider_auth_error,
    format_provider_auth_command, google_adc_quota_project_command,
};
use mcp_toolkit_auth::upstream_oauth::{
    UpstreamOAuthError, google_authorized_user_adc_metadata_from_file,
};
use serde::Serialize;
use tokio::process::Command;

use crate::client::auth_source_from_settings;
use crate::config::{
    AuthCommandArgs, AuthDoctorArgs, AuthLoginArgs, AuthStatusCliArgs, AuthSubcommand, Settings,
    conventional_adc_credentials_path, selected_adc_file, server_adc_credentials_path,
    server_cloudsdk_config_dir,
};
use crate::contract::redact_secret_text;
use crate::{AdManagerClient, AuthSource, MANAGE_SCOPE};

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
    let quota_project =
        effective_quota_project(settings, credential_status.adc_file.as_ref(), &env);
    let verification = if verify {
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
                    },
                    Err(err) => VerificationReport {
                        checked: true,
                        ok: Some(false),
                        sample_network_count: None,
                        error: Some(redact_secret_text(&err.to_string())),
                        hint: Some(err.hint().to_string()),
                    },
                }
            }
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

    let config_issue = credential_status.config_issue.clone().or_else(|| {
        credential_source
            .as_ref()
            .err()
            .map(|err| redact_secret_text(&err.to_string()))
    });
    let config_valid = credential_status.config_valid && credential_source.is_ok();
    let ready = if !config_valid {
        "no".to_string()
    } else {
        match verification.ok {
            Some(true) => "yes",
            Some(false) => "no",
            None => "not_verified",
        }
        .to_string()
    };
    let credential_material_detected =
        credential_status.credential_material_detected || verification.ok == Some(true);
    let next_steps = next_steps(settings, &quota_project, &verification, &credential_status);

    AuthReport {
        server: "google-ad-manager-mcp",
        scope: settings.scope.clone(),
        credential_source: preferred_auth_source(settings, &env),
        config_valid,
        config_issue,
        credential_material_detected,
        quota_project,
        gcloud: gcloud_version().await,
        adc_file: credential_status.adc_file,
        env,
        verification,
        ready,
        next_steps,
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
    verification: &VerificationReport,
    credential_status: &CredentialSourceStatus,
) -> Vec<String> {
    let mut steps = Vec::new();
    if let Some(step) = &credential_status.repair_step {
        steps.push(step.clone());
    }
    if !verification.checked {
        let step = if credential_status.config_valid {
            "Run `google-ad-manager-mcp auth status --verify-token` when you are ready to prove access."
        } else {
            "After fixing the credential configuration, run `google-ad-manager-mcp auth status --verify-token` to prove access."
        };
        steps.push(step.to_string());
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
    match CustomServiceAccount::from_file(path.display().to_string()) {
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
                        .to_string()
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
                    credential_material_detected: true,
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

fn uses_local_user_adc(env: &EnvStatus) -> bool {
    !env.google_application_credentials && !env.service_account_path && !env.service_account_json
}

fn preferred_auth_source(settings: &Settings, env: &EnvStatus) -> AuthSource {
    if settings.service_account_json.is_some() {
        AuthSource::ServiceAccountJsonEnv
    } else if settings.service_account_json_path.is_some() {
        AuthSource::ServiceAccountJsonPath
    } else if env.google_application_credentials {
        AuthSource::GoogleDefaultProviderChain
    } else {
        AuthSource::GoogleAuthorizedUserAdcFile
    }
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

#[derive(Debug, Serialize)]
struct VerificationReport {
    checked: bool,
    ok: Option<bool>,
    sample_network_count: Option<usize>,
    error: Option<String>,
    hint: Option<String>,
}

#[derive(Debug)]
struct CredentialSourceStatus {
    config_valid: bool,
    config_issue: Option<String>,
    credential_material_detected: bool,
    repair_step: Option<String>,
    adc_file: Option<AdcFileStatus>,
}

#[cfg(test)]
mod tests {
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    use std::path::Path;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        auth_command_shared_adc, gcloud_adc_login_command, google_application_credentials_status,
        shell_join, shell_join_with_cloudsdk_config,
    };
    use crate::Settings;

    const TEST_SERVICE_ACCOUNT_PRIVATE_KEY: &str = "-----BEGIN PRIVATE KEY-----\n\
MIICdQIBADANBgkqhkiG9w0BAQEFAASCAl8wggJbAgEAAoGBAPnRvYZzxdotNxOS\n\
kDEYigDqPmk/+JTMpLBSvzQ55uASv5fsPUYNb+Pje+KwrVfEqq/tI/Nz4mOgKeV2\n\
xGD7XhUzvuFLWNflfp0R93MI1qKC+onD7q0WsakpH0miXjHj6yZ7rHVne3E5o3ip\n\
LNuP/q89l6UcjBkMfgfs/osRUi+zAgMBAAECgYA4AREf5yxfsOs79AtnNj0Z32mG\n\
ZtTvZsE01hgPOTvM1+cjw84oujJvQDwxobH6jxhEwEDi/wOtmeZKjsmPhEqevMpi\n\
9DjLL3w3k3pRwoddRnERWpQTV/37YJ3VGczKji6tQKTFm8H6NQt/Cs2MAwayQdU2\n\
jF/QdL7ysv0WjUyIQQJBAP9zK99X/+0TEWQAYk6EKwu0oEfqZVWO2crPEEhQ2mvB\n\
CiXQ2Lg1LFScGCDNVQdgh5t5D77NmVWxyxDJ/XOrRukCQQD6W3b/NZ19NG5Xk5XE\n\
IUNwnGkbddqtHA9x8nNYw7wPqoD9p6XgI83eKzNCPzTWfiQjoJAuTvfLKPIKayte\n\
uxg7Aj8h7Snmf8l9swqcPXDQ/Ly60UJ4Sqkqs845IUcIU7SumvS+EP63eFhq5FBQ\n\
CvVABZH9FBcDQEsdFn/huvHuatECQA0Tf+iehUZH2ceLNtRSpHIaSUcc5boK8CeU\n\
cT/eoVD0J96Xxgsp85O6D+hS4tCdMAgIV9+DUl/zGIlAxbgh74cCQQDesPGAE2ZG\n\
QO/f5L4azuE8yAB9ob3w4K9ZovtbjqTUz7vW4SRwDNXgXW/hCungTLb5hVpfYxPf\n\
rFCaohNaJ5PK\n\
-----END PRIVATE KEY-----\n";

    fn test_service_account_json() -> String {
        format!(
            r#"{{
  "type": "service_account",
  "project_id": "test-project",
  "private_key_id": "test-key-id",
  "private_key": {private_key:?},
  "client_email": "test-service-account@example.iam.gserviceaccount.com",
  "client_id": "123456789012345678901",
  "auth_uri": "https://accounts.google.com/o/oauth2/auth",
  "token_uri": "https://oauth2.googleapis.com/token",
  "auth_provider_x509_cert_url": "https://www.googleapis.com/oauth2/v1/certs",
  "client_x509_cert_url": "https://www.googleapis.com/robot/v1/metadata/x509/test-service-account%40example.iam.gserviceaccount.com"
}}"#,
            private_key = TEST_SERVICE_ACCOUNT_PRIVATE_KEY,
        )
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
        let settings = Settings {
            shared_adc: true,
            ..Settings::default()
        };

        assert!(auth_command_shared_adc(&settings, false));
        assert!(auth_command_shared_adc(&settings, true));
        assert!(auth_command_shared_adc(&Settings::default(), true));
        assert!(!auth_command_shared_adc(&Settings::default(), false));
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
        assert!(
            status
                .repair_step
                .as_deref()
                .is_some_and(|step| step.contains("Unset `GOOGLE_APPLICATION_CREDENTIALS`"))
        );

        let _ = fs::remove_file(path);
    }

    #[cfg(unix)]
    #[test]
    fn google_application_credentials_accepts_runtime_compatible_service_account_symlink() {
        let target = unique_test_file("google-application-credentials-service-account", "json");
        let link = unique_test_file(
            "google-application-credentials-service-account-link",
            "json",
        );
        fs::create_dir_all(target.parent().expect("test file parent")).expect("create test dir");
        fs::write(&target, test_service_account_json()).expect("write service-account json");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).expect("chmod");
        }
        let target = fs::canonicalize(&target).expect("canonicalize target");
        symlink(&target, &link).expect("create symlink");

        let status = google_application_credentials_status(link.clone());

        assert!(status.config_valid);
        assert!(status.config_issue.is_none());
        assert!(status.credential_material_detected);
        assert!(status.repair_step.is_none());

        let _ = fs::remove_file(link);
        let _ = fs::remove_file(target);
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
