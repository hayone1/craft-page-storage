//! The page's answers, read into the contract's types.
//!
//! The answers arrive here as JSON (`page` turns the JavaScript values into it first), so these
//! rules run and are tested on every target. Bytes never come through here.

use craft_cloud::{Capability, Entry, EntryKind};
use craft_ui::cloud::{Account, Fault};
use serde_json::Value;

/// One file or folder the page described. Only `id` is required.
pub(crate) fn entry(v: &Value) -> Result<Entry, String> {
    if !v.is_object() {
        return Err(format!("{v} is not a file"));
    }
    let text = |key: &str| v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned);
    let id = text("id").ok_or_else(|| format!("{v} has no id"))?;
    let kind = match v.get("kind").and_then(Value::as_str) {
        None | Some("file") => EntryKind::File,
        Some("folder") => EntryKind::Folder,
        Some(other) => return Err(format!("{id} is of an unknown kind, {other:?}")),
    };
    Ok(Entry {
        name: text("name").unwrap_or_else(|| last_part(&id).to_owned()),
        room: text("room").unwrap_or_else(|| id.clone()),
        // The Open page browses into a folder by its path, so it can't be empty.
        path: text("path").unwrap_or_else(|| id.clone()),
        kind,
        size: v.get("size").and_then(Value::as_u64),
        modified: text("modified"),
        mime: text("mime"),
        etag: text("etag"),
        writable: v.get("writable").and_then(Value::as_bool).unwrap_or(true),
        shared_by: text("sharedBy"),
        id,
    })
}

/// A folder's contents, or what was shared.
pub(crate) fn entries(v: &Value) -> Result<Vec<Entry>, String> {
    v.as_array().ok_or_else(|| format!("{v} is not a list of files"))?.iter().map(entry).collect()
}

/// Who is signed in. `shared` says whether the page has `sharedWithMe`.
///
/// Only the capabilities this backend serves are kept. Sharing, contacts and conversations go
/// through other backends, and the pages would offer a control for them that fails.
pub(crate) fn account(v: &Value, shared: bool) -> Result<Account, String> {
    if !v.is_object() {
        return Err(format!("{v} is not an account"));
    }
    let listed: Vec<Capability> =
        v.get("capabilities").and_then(Value::as_array).into_iter().flatten().filter_map(|c| serde_json::from_value(c.clone()).ok()).collect();
    let mut capabilities = vec![Capability::Files];
    if shared {
        capabilities.push(Capability::SharedWithMe);
    }
    if listed.contains(&Capability::Etags) {
        capabilities.push(Capability::Etags);
    }
    let display = v.get("display").and_then(Value::as_str).unwrap_or_default().to_owned();
    // The page's storage is there as soon as the page is: there is no account to link.
    Ok(Account { display, capabilities, provider_linked: true })
}

/// A failed call, by the `code` the page gave it.
pub(crate) fn fault(code: Option<&str>, message: Option<String>) -> Fault {
    let message = message.filter(|m| !m.trim().is_empty());
    match code {
        Some("conflict") => Fault::Conflict,
        Some("unauthenticated") => Fault::Unauthenticated,
        Some("not_linked") => Fault::NotLinked,
        Some("unreachable") => Fault::Unreachable(message.unwrap_or_default()),
        _ => Fault::Refused(message.unwrap_or_else(|| "The page's storage refused.".to_owned())),
    }
}

/// What to call a file the page gave no name: the last part of its id.
fn last_part(id: &str) -> &str {
    id.rsplit(['/', '\\', ':']).find(|part| !part.is_empty()).unwrap_or(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_entry_needs_only_an_id() {
        let e = entry(&json!({"id": "cloud:home/Plans/a.psd"})).unwrap();
        assert_eq!((e.name.as_str(), e.room.as_str(), e.path.as_str()), ("a.psd", "cloud:home/Plans/a.psd", "cloud:home/Plans/a.psd"));
        assert_eq!(e.kind, EntryKind::File);
        assert!(e.writable);
        assert_eq!((e.size, e.etag, e.shared_by), (None, None, None));
    }

    #[test]
    fn an_entry_keeps_what_the_page_said() {
        let e = entry(&json!({
            "id": "42", "name": "Plans", "path": "Team/Plans", "kind": "folder", "size": 7, "modified": "2026-10-29T10:00:00Z",
            "mime": "image/png", "etag": "\"v2\"", "writable": false, "room": "r42", "sharedBy": "Ada",
        }))
        .unwrap();
        assert_eq!(e.kind, EntryKind::Folder);
        assert_eq!((e.name.as_str(), e.path.as_str(), e.room.as_str()), ("Plans", "Team/Plans", "r42"));
        assert_eq!((e.size, e.etag.as_deref(), e.shared_by.as_deref()), (Some(7), Some("\"v2\""), Some("Ada")));
        assert_eq!((e.modified.as_deref(), e.mime.as_deref()), (Some("2026-10-29T10:00:00Z"), Some("image/png")));
        assert!(!e.writable);
    }

    #[test]
    fn a_bad_entry_is_an_error() {
        for bad in [json!(null), json!("a.psd"), json!({}), json!({"id": ""}), json!({"id": 7}), json!({"id": "a", "kind": "link"})] {
            assert!(entry(&bad).is_err(), "{bad}");
        }
        assert!(entries(&json!({"id": "a"})).is_err());
        assert!(entries(&json!([{"id": "a"}, {}])).is_err());
        assert_eq!(entries(&json!([{"id": "a"}, {"id": "b"}])).unwrap().len(), 2);
    }

    #[test]
    fn an_account_keeps_only_what_this_backend_serves() {
        let a = account(&json!({"display": "Ada", "capabilities": ["sharing", "etags", "contacts", "no such thing"]}), false).unwrap();
        assert_eq!(a.display, "Ada");
        assert_eq!(a.capabilities, [Capability::Files, Capability::Etags]);
        assert!(a.provider_linked);
        let a = account(&json!({}), true).unwrap();
        assert_eq!(a.capabilities, [Capability::Files, Capability::SharedWithMe]);
        assert!(account(&json!("Ada"), false).is_err());
    }

    #[test]
    fn a_code_picks_the_fault() {
        assert_eq!(fault(Some("conflict"), Some("changed".into())), Fault::Conflict);
        assert_eq!(fault(Some("unauthenticated"), None), Fault::Unauthenticated);
        assert_eq!(fault(Some("not_linked"), None), Fault::NotLinked);
        assert_eq!(fault(Some("unreachable"), Some("offline".into())), Fault::Unreachable("offline".into()));
        assert_eq!(fault(Some("forbidden"), Some("read only".into())), Fault::Refused("read only".into()));
        assert_eq!(fault(None, Some("  ".into())), Fault::Refused("The page's storage refused.".into()));
    }
}
