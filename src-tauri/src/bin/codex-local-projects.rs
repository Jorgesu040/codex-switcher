//! One-off experiment: reassign project configuration to the current account.
use anyhow::{bail, Context, Result};
use codex_switcher_lib::auth::{get_codex_home, local_projects, read_current_auth};

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|s| s == "--help") {
        println!("codex-local-projects [--apply] [--source-owner HASH]\ncodex-local-projects --restore BACKUP_DIRECTORY\nDefault: preview only. Close Codex before --apply or --restore.");
        return Ok(());
    }
    let mut apply = false;
    let mut source = None;
    let mut restore = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--apply" => apply = true,
            "--source-owner" => {
                i += 1;
                source = Some(args.get(i).context("Missing source owner")?.as_str());
            }
            "--restore" => {
                i += 1;
                restore = Some(std::path::PathBuf::from(
                    args.get(i).context("Missing backup directory")?,
                ));
            }
            other => bail!("Unknown argument: {other}"),
        }
        i += 1;
    }
    if restore.is_some() && (apply || source.is_some()) {
        bail!("Use --restore separately");
    }
    let home = get_codex_home()?;
    if apply || restore.is_some() {
        let processes = codex_switcher_lib::commands::check_codex_processes()
            .await
            .map_err(anyhow::Error::msg)?;
        if !processes.can_switch {
            bail!(
                "Close Codex completely before changing its configuration ({} active processes)",
                processes.count
            );
        }
    }
    if let Some(backup) = restore {
        let root = home.join("project-rebind-backups").canonicalize()?;
        let backup = backup.canonicalize()?;
        if backup.parent() != Some(root.as_path()) {
            bail!("Backup must be inside this Codex home's project-rebind-backups directory");
        }
        local_projects::restore(&home, &backup)?;
        println!("Original configuration restored.");
        return Ok(());
    }
    let auth = read_current_auth()?.context("No current login")?;
    let tokens = auth
        .tokens
        .context("This experiment requires a ChatGPT account")?;
    let owner = local_projects::renderer_owner(&tokens.id_token, tokens.account_id.as_deref())?;
    let bytes = std::fs::read(home.join(".codex-global-state.json"))?;
    let state = serde_json::from_slice(&bytes)?;
    let plan = local_projects::plan_rebind(&state, &owner, source)?;
    println!(
        "Source owner: {}\nCurrent account owner: {}\nLocal projects: {}",
        plan.source_owner, plan.target_owner, plan.project_count
    );
    if apply {
        match local_projects::apply_plan(&home, &bytes, &plan)? {
            Some(backup) => println!("Reassigned local project configuration.\nBackup: {}\nReopen Codex to check the sidebar.", backup.display()),
            None => println!("Already assigned; nothing changed."),
        }
    } else {
        println!("Preview only. No files changed. Use --apply with Codex closed to test.");
    }
    Ok(())
}
