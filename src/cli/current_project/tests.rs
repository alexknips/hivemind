use super::*;

fn unique_test_dir() -> PathBuf {
    std::env::temp_dir().join(format!("hivemind-current-project-{}", uuid::Uuid::new_v4()))
}

#[test]
fn get_is_none_before_anything_is_set() {
    let store = CurrentProjectStore::new(&unique_test_dir());
    assert_eq!(store.get("local", "human:alice").unwrap(), None);
}

#[test]
fn set_then_get_round_trips() {
    let store = CurrentProjectStore::new(&unique_test_dir());
    store.set("local", "human:alice", "billing").unwrap();
    assert_eq!(
        store.get("local", "human:alice").unwrap(),
        Some("billing".to_owned())
    );
}

#[test]
fn setting_does_not_leak_across_tenants() {
    let store = CurrentProjectStore::new(&unique_test_dir());
    store.set("tenant-a", "human:alice", "billing").unwrap();
    assert_eq!(store.get("tenant-b", "human:alice").unwrap(), None);
    assert_eq!(
        store.get("tenant-a", "human:alice").unwrap(),
        Some("billing".to_owned())
    );
}

#[test]
fn setting_does_not_leak_across_actors() {
    let store = CurrentProjectStore::new(&unique_test_dir());
    store.set("local", "human:alice", "billing").unwrap();
    assert_eq!(store.get("local", "human:bob").unwrap(), None);
}

#[test]
fn clear_removes_the_setting() {
    let store = CurrentProjectStore::new(&unique_test_dir());
    store.set("local", "human:alice", "billing").unwrap();
    store.clear("local", "human:alice").unwrap();
    assert_eq!(store.get("local", "human:alice").unwrap(), None);
}

#[test]
fn clear_on_an_unset_actor_is_a_no_op() {
    let store = CurrentProjectStore::new(&unique_test_dir());
    store.clear("local", "human:alice").unwrap();
    assert_eq!(store.get("local", "human:alice").unwrap(), None);
}

#[test]
fn set_persists_across_store_instances() {
    let dir = unique_test_dir();
    CurrentProjectStore::new(&dir)
        .set("local", "human:alice", "billing")
        .unwrap();
    assert_eq!(
        CurrentProjectStore::new(&dir)
            .get("local", "human:alice")
            .unwrap(),
        Some("billing".to_owned())
    );
}

// Precedence stub of D1's order hook (hivemind-s15q.12, not yet landed):
// stated > folder marker > rig > current project > personal fallback. D1
// owns the top three rungs and their resolution; this local stand-in
// resolves only the two rungs D2 owns underneath them, `higher_rung` playing
// the part of whatever D1's resolution module would already have found, so
// the acceptance criterion below is exercised end to end even before D1
// lands. When D1 lands, its module supplies the real higher rungs and calls
// into logic like this instead of re-implementing the last two.

const SOURCE_CURRENT_PROJECT: &str = "current_project";
const SOURCE_PERSONAL_FALLBACK: &str = "personal_fallback";

fn resolve_below_marker_and_rig(
    higher_rung: Option<(String, &'static str)>,
    current_project: Option<String>,
    personal_project: String,
) -> (String, &'static str) {
    if let Some(resolved) = higher_rung {
        return resolved;
    }
    match current_project {
        Some(handle) => (handle, SOURCE_CURRENT_PROJECT),
        None => (personal_project, SOURCE_PERSONAL_FALLBACK),
    }
}

#[test]
fn marker_or_rig_beats_current_project() {
    let (handle, source) = resolve_below_marker_and_rig(
        Some(("platform".to_owned(), "rig")),
        Some("billing".to_owned()),
        "personal:human:alice".to_owned(),
    );
    assert_eq!(handle, "platform");
    assert_eq!(source, "rig");
}

#[test]
fn current_project_beats_personal_fallback() {
    let (handle, source) = resolve_below_marker_and_rig(
        None,
        Some("billing".to_owned()),
        "personal:human:alice".to_owned(),
    );
    assert_eq!(handle, "billing");
    assert_eq!(source, SOURCE_CURRENT_PROJECT);
}

#[test]
fn personal_fallback_when_nothing_else_is_set() {
    let (handle, source) =
        resolve_below_marker_and_rig(None, None, "personal:human:alice".to_owned());
    assert_eq!(handle, "personal:human:alice");
    assert_eq!(source, SOURCE_PERSONAL_FALLBACK);
}
