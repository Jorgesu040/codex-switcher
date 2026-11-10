//! Experimental reassignment of LOCAL project configuration, never credentials or chats.
//! Callers must ensure Codex is closed before committing a plan.

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

const REGISTRY: &str = "electron-account-renderer-state-v1";
const STATE: &str = ".codex-global-state.json";

#[derive(Debug)]
pub struct RebindPlan {
    pub state: Value,
    pub source_owner: String,
    pub target_owner: String,
    pub project_count: usize,
}

/// Match the desktop's SHA256(JSON.stringify([userId, accountId])). No token is changed.
pub fn renderer_owner(id_token: &str, account_id: Option<&str>) -> Result<String> {
    let payload = id_token
        .split('.')
        .nth(1)
        .context("Missing identity token payload")?;
    let claims: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload)?)?;
    let auth = &claims["https://api.openai.com/auth"];
    let user = auth["chatgpt_user_id"]
        .as_str()
        .or_else(|| auth["user_id"].as_str())
        .filter(|s| !s.is_empty())
        .context("Token has no ChatGPT user identifier")?;
    let account = account_id
        .or_else(|| auth["chatgpt_account_id"].as_str())
        .filter(|s| !s.is_empty())
        .context("Token has no ChatGPT account identifier")?;
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&json!([user, account]))?)
    ))
}

fn local_projects(globals: &Value) -> Map<String, Value> {
    globals["local-projects"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(id, p)| {
            !id.starts_with("g-p-")
                && p["rootPaths"].as_array().is_some_and(|roots| {
                    roots.iter().all(|r| {
                        r.as_str().is_some_and(|s| {
                            !s.replace('\\', "/")
                                .to_lowercase()
                                .contains("/.chatgpt-projects/")
                        })
                    })
                })
        })
        .map(|(id, p)| (id.clone(), p.clone()))
        .collect()
}

fn merge_map(target: &mut Value, key: &str, source: Map<String, Value>) -> Result<()> {
    if source.is_empty() {
        return Ok(());
    }
    let target = target
        .as_object_mut()
        .context("Malformed target globals")?
        .entry(key)
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .with_context(|| format!("Malformed target field: {key}"))?;
    // Existing target values win; another account's projects are never removed.
    for (k, v) in source {
        target.entry(k).or_insert(v);
    }
    Ok(())
}

fn append_values(target: &mut Value, key: &str, values: impl Iterator<Item = Value>) -> Result<()> {
    let values: Vec<_> = values.collect();
    if values.is_empty() {
        return Ok(());
    }
    let array = target
        .as_object_mut()
        .context("Malformed target globals")?
        .entry(key)
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .with_context(|| format!("Malformed target field: {key}"))?;
    for v in values {
        if !array.contains(&v) {
            array.push(v);
        }
    }
    Ok(())
}

pub fn plan_rebind(
    original: &Value,
    target_owner: &str,
    source_owner: Option<&str>,
) -> Result<RebindPlan> {
    if target_owner.len() != 64 || !target_owner.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("Invalid renderer owner identifier");
    }
    let registry = original[REGISTRY]
        .as_object()
        .context("Codex has no account-scoped project registry")?;
    let owner = registry.get("owner").and_then(Value::as_str);
    let inactive = registry
        .get("inactive")
        .and_then(Value::as_object)
        .context("Malformed inactive account registry")?;
    let mut candidates: Vec<(String, Value)> = Vec::new();
    if let Some(owner) = owner {
        if !local_projects(original).is_empty() {
            candidates.push((owner.into(), original.clone()));
        }
    }
    for (id, snapshot) in inactive {
        if !local_projects(&snapshot["globals"]).is_empty() {
            candidates.push((id.clone(), snapshot["globals"].clone()));
        }
    }
    let source = if let Some(id) = source_owner {
        candidates
            .iter()
            .find(|(k, _)| k == id)
            .context("Source owner has no local projects")?
    } else if let Some(current) = candidates.iter().find(|(k, _)| Some(k.as_str()) == owner) {
        current
    } else {
        if candidates.len() != 1 {
            bail!(
                "Expected one source with local projects, found {}; use --source-owner HASH",
                candidates.len()
            );
        }
        &candidates[0]
    };
    let projects = local_projects(&source.1);
    let mut ids: HashSet<String> = projects.keys().cloned().collect();
    let mut host_maps = Map::new();
    if let Some(hosts) = source.1["app-server-project-id-by-legacy-project-id-by-host"].as_object()
    {
        for (host, map) in hosts.iter().filter(|(h, _)| h.starts_with("local:")) {
            let entries: Map<_, _> = map
                .as_object()
                .context("Malformed local project ID map")?
                .iter()
                .filter(|(id, _)| ids.contains(*id))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            host_maps.insert(host.clone(), Value::Object(entries));
        }
    }
    for map in host_maps.values() {
        for id in map.as_object().unwrap().values().filter_map(Value::as_str) {
            ids.insert(id.into());
        }
    }
    let mut state = original.clone();
    let mut target = if owner == Some(target_owner) {
        state.clone()
    } else {
        inactive
            .get(target_owner)
            .and_then(|s| s.get("globals"))
            .cloned()
            .unwrap_or_else(|| json!({}))
    };
    let before = target.clone();
    merge_map(&mut target, "local-projects", projects.clone())?;
    for key in [
        "project-appearances",
        "project-writable-roots",
        "sidebar-project-thread-orders",
    ] {
        let entries = source.1[key]
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(id, _)| ids.contains(*id))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        merge_map(&mut target, key, entries)?;
    }
    for (host, map) in &host_maps {
        let hosts = target
            .as_object_mut()
            .unwrap()
            .entry("app-server-project-id-by-legacy-project-id-by-host")
            .or_insert_with(|| json!({}));
        merge_map(hosts, host, map.as_object().unwrap().clone())?;
    }
    let assignments: Map<_, _> = source.1["thread-project-assignments"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(thread, v)| {
            v["projectKind"] == "local"
                && source.1["thread-project-membership-host-ids"]
                    .get(*thread)
                    .and_then(Value::as_str)
                    .is_none_or(|host| host == "local")
                && v["projectId"].as_str().is_some_and(|id| ids.contains(id))
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for key in [
        "thread-project-membership-host-ids",
        "thread-writable-roots",
        "thread-workspace-root-hints",
    ] {
        let entries = source.1[key]
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(id, _)| assignments.contains_key(*id))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        merge_map(&mut target, key, entries)?;
    }
    merge_map(
        &mut target,
        "thread-project-assignments",
        assignments.clone(),
    )?;
    for key in ["project-order", "pinned-project-ids"] {
        append_values(
            &mut target,
            key,
            source.1[key]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|v| v.as_str().is_some_and(|s| ids.contains(s)))
                .cloned(),
        )?;
    }
    append_values(
        &mut target,
        "electron-saved-workspace-roots",
        projects
            .values()
            .flat_map(|p| p["rootPaths"].as_array().into_iter().flatten())
            .cloned(),
    )?;
    if before != target {
        // Force the desktop's idempotent migration to reconsider the restored local IDs.
        // Preserve migration state for every remote host.
        let migrations = target
            .as_object_mut()
            .unwrap()
            .entry("app-server-projects-migration-by-host")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .context("Malformed migration registry")?;
        for host in host_maps.keys() {
            let record = migrations
                .entry(host)
                .or_insert_with(|| json!({"version":1}));
            if record["version"].as_u64() != Some(1) {
                bail!("Unsupported local project migration version");
            }
            append_values(
                record,
                "pendingThreadAssignmentIds",
                assignments.keys().cloned().map(Value::String),
            )?;
            record["projectsMigrated"] = Value::Bool(false);
            record["threadAssignmentsMigrated"] = Value::Bool(false);
        }
    }
    if owner == Some(target_owner) {
        // Copy only the changed local fields, not the nested registry or account atoms.
        for (key, value) in target.as_object().unwrap() {
            if before.get(key) != Some(value) {
                state[key] = value.clone();
            }
        }
    } else {
        let snapshots = state[REGISTRY]["inactive"].as_object_mut().unwrap();
        let snapshot = snapshots
            .entry(target_owner)
            .or_insert_with(|| json!({"atoms":{},"globals":{}}));
        if !snapshot["atoms"].is_object() {
            bail!("Malformed target account atoms");
        }
        snapshot["globals"] = target;
    }
    Ok(RebindPlan {
        state,
        source_owner: source.0.clone(),
        target_owner: target_owner.into(),
        project_count: projects.len(),
    })
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn replace(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension(format!("rebind-{}.tmp", uuid::Uuid::new_v4()));
    write_private(&tmp, bytes)?;
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(tmp);
        return Err(e.into());
    }
    Ok(())
}

/// Store exact original configuration bytes, then atomically replace each state file.
pub fn apply_plan(
    codex_home: &Path,
    original_bytes: &[u8],
    plan: &RebindPlan,
) -> Result<Option<PathBuf>> {
    let original: Value = serde_json::from_slice(original_bytes)?;
    if original == plan.state {
        return Ok(None);
    }
    let path = codex_home.join(STATE);
    if fs::read(&path)? != original_bytes {
        bail!("Codex configuration changed; close Codex and retry");
    }
    let backup = codex_home
        .join("project-rebind-backups")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&backup)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&backup, fs::Permissions::from_mode(0o700))?;
    }
    write_private(&backup.join("state.json"), original_bytes)?;
    let bak_path = codex_home.join(format!("{STATE}.bak"));
    let old_bak = match fs::read(&bak_path) {
        Ok(bytes) => {
            write_private(&backup.join("state.bak.json"), &bytes)?;
            Some(bytes)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    let bytes = serde_json::to_vec(&plan.state)?;
    if fs::read(&path)? != original_bytes {
        bail!("Codex configuration changed before commit; retry with Codex closed");
    }
    replace(&path, &bytes)?;
    if let Err(e) = replace(&bak_path, &bytes) {
        let _ = replace(&path, original_bytes);
        if let Some(bytes) = old_bak {
            let _ = replace(&bak_path, &bytes);
        }
        return Err(e.context("Failed to update fallback configuration; attempted rollback"));
    }
    Ok(Some(backup))
}

pub fn restore(codex_home: &Path, backup: &Path) -> Result<()> {
    let bytes = fs::read(backup.join("state.json"))?;
    let _: Value = serde_json::from_slice(&bytes)?;
    let path = codex_home.join(STATE);
    let bak_path = codex_home.join(format!("{STATE}.bak"));
    // Keep the current state too, so an undo can itself be undone.
    let current = fs::read(&path)?;
    // apply_plan handles exact backups and both destination files.
    let plan = RebindPlan {
        state: serde_json::from_slice(&bytes)?,
        source_owner: String::new(),
        target_owner: String::new(),
        project_count: 0,
    };
    let _ = apply_plan(codex_home, &current, &plan)?;
    // Preserve formatting and restore the original fallback file exactly.
    replace(&path, &bytes)?;
    let old_bak = backup.join("state.bak.json");
    if old_bak.exists() {
        replace(&bak_path, &fs::read(old_bak)?)?;
    } else if bak_path.exists() {
        fs::remove_file(bak_path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const OLD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const NEW: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    fn fixture() -> Value {
        json!({
            REGISTRY: {"owner":NEW,"inactive":{OLD:{"atoms":{"old-cloud-cache":"private"},"globals":{
                "local-projects":{"local-one":{"id":"local-one","name":"Example","rootPaths":["C:\\Projects\\example"]},
                    "g-p-cloud":{"rootPaths":["C:\\Users\\test\\.codex\\.chatgpt-projects\\g-p-cloud"]}},
                "app-server-project-id-by-legacy-project-id-by-host":{"local:C:\\test\\.codex":{"local-one":"server-one","g-p-cloud":"server-cloud"},"remote:ssh":{"remote":"remote-id"}},
                "thread-project-assignments":{"local-chat":{"projectKind":"local","projectId":"local-one"},"cloud-chat":{"projectKind":"chatgpt","projectId":"g-p-cloud"},"remote-chat":{"projectKind":"local","projectId":"local-one"}},
                "thread-project-membership-host-ids":{"local-chat":"local","remote-chat":"ssh-host"},
                "project-order":["local-one","g-p-cloud"], "pinned-project-ids":["server-one","g-p-cloud"],
                "project-appearances":{"local-one":{"color":"blue"},"g-p-cloud":{"color":"red"}},
                "remote-projects":["private-remote"], "projectless-thread-ids":["unrelated-chat"]
            }}}},
            "electron-persisted-atom-state":{"cloud-cache":"current"},
            "thread-project-assignments":{"my-cloud":{"projectKind":"chatgpt","projectId":"g-p-new"}},
            "app-server-projects-migration-by-host":{"remote:ssh":{"projectsMigrated":true}},
            "remote-projects":["current-remote"]
        })
    }
    #[test]
    fn recovers_local_projects_for_current_owner_without_moving_cloud_or_remote_data() {
        let original = fixture();
        let p = plan_rebind(&original, NEW, None).unwrap();
        assert_eq!(p.project_count, 1);
        assert_eq!(p.state["local-projects"]["local-one"]["name"], "Example");
        assert!(p.state["local-projects"].get("g-p-cloud").is_none());
        assert_eq!(
            p.state["electron-persisted-atom-state"],
            original["electron-persisted-atom-state"]
        );
        assert_eq!(p.state[REGISTRY], original[REGISTRY]);
        assert_eq!(p.state["remote-projects"], original["remote-projects"]);
        assert!(p.state["thread-project-assignments"]
            .get("my-cloud")
            .is_some());
        assert!(p.state["thread-project-assignments"]
            .get("local-chat")
            .is_some());
        assert!(p.state["thread-project-assignments"]
            .get("cloud-chat")
            .is_none());
        assert!(p.state["thread-project-assignments"]
            .get("remote-chat")
            .is_none());
        assert_eq!(p.state["pinned-project-ids"], json!(["server-one"]));
        assert_eq!(
            p.state["app-server-projects-migration-by-host"]["remote:ssh"],
            original["app-server-projects-migration-by-host"]["remote:ssh"]
        );
        assert_eq!(
            p.state["app-server-projects-migration-by-host"]["local:C:\\test\\.codex"]
                ["projectsMigrated"],
            false
        );
    }
    #[test]
    fn switch_prepares_target_snapshot_and_preserves_its_account_atoms() {
        let mut original = fixture();
        original[REGISTRY]["owner"] = Value::String(OLD.into());
        original["local-projects"] =
            original[REGISTRY]["inactive"][OLD]["globals"]["local-projects"].clone();
        original[REGISTRY]["inactive"][NEW] =
            json!({"atoms":{"target-cloud":"keep"},"globals":{"other":"keep"}});
        let p = plan_rebind(&original, NEW, None).unwrap();
        assert_eq!(p.state[REGISTRY]["owner"], OLD);
        assert_eq!(p.state["local-projects"], original["local-projects"]);
        assert_eq!(
            p.state[REGISTRY]["inactive"][NEW]["atoms"],
            json!({"target-cloud":"keep"})
        );
        assert_eq!(
            p.state[REGISTRY]["inactive"][NEW]["globals"]["other"],
            "keep"
        );
        assert!(
            p.state[REGISTRY]["inactive"][NEW]["globals"]["local-projects"]
                .get("local-one")
                .is_some()
        );
        assert_eq!(
            p.state[REGISTRY]["inactive"][OLD],
            original[REGISTRY]["inactive"][OLD]
        );
    }
    #[test]
    fn existing_target_projects_and_overrides_win_and_repeating_is_idempotent() {
        let mut original = fixture();
        original["local-projects"] = json!({"local-one":{"id":"local-one","name":"Target name","rootPaths":[]},"local-two":{"rootPaths":[]}});
        original["thread-project-assignments"]["target-only-chat"] =
            json!({"projectKind":"local","projectId":"local-one"});
        original["app-server-projects-migration-by-host"]["local:C:\\test\\.codex"] = json!({"version":1,"projectsMigrated":false,"threadAssignmentsMigrated":false,"pendingThreadAssignmentIds":["target-only-pending"],"extra":"keep"});
        let p = plan_rebind(&original, NEW, Some(OLD)).unwrap();
        assert_eq!(
            p.state["local-projects"]["local-one"]["name"],
            "Target name"
        );
        assert!(p.state["local-projects"].get("local-two").is_some());
        assert_eq!(
            p.state["thread-project-assignments"]["target-only-chat"],
            original["thread-project-assignments"]["target-only-chat"]
        );
        let migration = &p.state["app-server-projects-migration-by-host"]["local:C:\\test\\.codex"];
        assert_eq!(
            migration["pendingThreadAssignmentIds"],
            json!(["target-only-pending", "local-chat"])
        );
        assert_eq!(migration["extra"], "keep");
        let repeated = plan_rebind(&p.state, NEW, Some(OLD)).unwrap();
        assert_eq!(p.state, repeated.state);
    }
    #[test]
    fn ambiguous_inactive_sources_require_explicit_selection() {
        let mut original = fixture();
        original[REGISTRY]["inactive"]
            ["cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"] =
            original[REGISTRY]["inactive"][OLD].clone();
        assert!(plan_rebind(&original, NEW, None)
            .unwrap_err()
            .to_string()
            .contains("Expected one source"));
        assert!(plan_rebind(&original, NEW, Some(OLD)).is_ok());
    }
    #[test]
    fn malformed_state_and_unknown_identity_fail_without_changes() {
        assert!(plan_rebind(&json!({}), NEW, None).is_err());
        assert!(plan_rebind(&fixture(), "invalid", None).is_err());
        assert!(renderer_owner("not-a-token", None).is_err());
        let mut original = fixture();
        original["local-projects"] = json!([]);
        assert!(plan_rebind(&original, NEW, None).is_err());
    }
    #[test]
    fn owner_matches_desktop_algorithm_and_auth_tokens_are_not_rewritten() {
        let claims = json!({"sub":"wrong-sub","https://api.openai.com/auth":{"chatgpt_user_id":"user-123","chatgpt_account_id":"acct-123"}});
        let token = format!(
            "header.{}.signature",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap())
        );
        let owner = renderer_owner(&token, None).unwrap();
        let expected = format!("{:x}", Sha256::digest(br#"["user-123","acct-123"]"#));
        assert_eq!(owner, expected);
        assert_ne!(
            renderer_owner(&token, Some("different-account")).unwrap(),
            owner
        );
    }
    #[test]
    fn exact_backups_restore_original_bytes_and_detect_concurrent_changes() {
        let dir = std::env::temp_dir().join(format!("codex-rebind-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        let bytes = serde_json::to_vec_pretty(&fixture()).unwrap();
        fs::write(dir.join(STATE), &bytes).unwrap();
        fs::write(dir.join(format!("{STATE}.bak")), b"{\"previous\":true}\n").unwrap();
        let p = plan_rebind(&fixture(), NEW, None).unwrap();
        let backup = apply_plan(&dir, &bytes, &p).unwrap().unwrap();
        assert_eq!(fs::read(backup.join("state.json")).unwrap(), bytes);
        assert_eq!(
            fs::read(dir.join(STATE)).unwrap(),
            fs::read(dir.join(format!("{STATE}.bak"))).unwrap()
        );
        assert!(apply_plan(&dir, &bytes, &p).is_err());
        restore(&dir, &backup).unwrap();
        assert_eq!(fs::read(dir.join(STATE)).unwrap(), bytes);
        assert_eq!(
            fs::read(dir.join(format!("{STATE}.bak"))).unwrap(),
            b"{\"previous\":true}\n"
        );
        // Only remove the unique temporary fixture directory made in this test.
        fs::remove_dir_all(&dir).unwrap();
    }
}
