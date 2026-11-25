use rmpkgx::{
    cleanup::{plan::Plan, safety, trash},
    core::{
        application::Application,
        association::{associate, Confidence},
        discovery, disk_usage, scanner,
    },
};
use std::{fs, sync::atomic::AtomicBool};
fn bundle(root: &std::path::Path, name: &str) -> Application {
    let path = root.join(name).with_extension("app");
    fs::create_dir_all(path.join("Contents")).unwrap();
    fs::write(path.join("Contents/Info.plist"), r#"<?xml version="1.0"?><plist version="1.0"><dict><key>CFBundleName</key><string>Example</string><key>CFBundleIdentifier</key><string>org.example.app</string><key>CFBundleShortVersionString</key><string>1.2</string></dict></plist>"#).unwrap();
    Application::read(&path).unwrap()
}
#[test]
fn metadata_discovery_duplicates_and_malformed() {
    let temp = tempfile::tempdir().unwrap();
    let app = bundle(temp.path(), "One");
    bundle(temp.path(), "Two");
    fs::create_dir(temp.path().join("Broken.app")).unwrap();
    let report = discovery::discover(&[temp.path().into()], &AtomicBool::new(false)).unwrap();
    assert_eq!(report.items.len(), 2);
    assert_eq!(report.issues.len(), 1);
    assert_eq!(app.version.as_deref(), Some("1.2"));
    assert!(discovery::resolve(&report.items, "Example").is_err());
    assert!(discovery::resolve(&report.items, app.path.to_str().unwrap()).is_ok());
}
#[test]
fn evidence_and_protected_paths() {
    let temp = tempfile::tempdir().unwrap();
    let app = bundle(temp.path(), "One");
    assert_eq!(
        associate(&app, "org.example.app.plist").unwrap().0,
        Confidence::High
    );
    assert_eq!(associate(&app, "Example").unwrap().0, Confidence::Medium);
    assert_eq!(associate(&app, "Example.data").unwrap().0, Confidence::Low);
    assert!(associate(&app, "ExampleOther").is_none());
    assert!(safety::protected(std::path::Path::new(
        "/System/Applications/Test.app"
    )));
    assert!(safety::validate(std::path::Path::new("/Users/a/../b")).is_err());
    assert!(safety::validate(std::path::Path::new("relative")).is_err());
}
#[test]
fn size_does_not_follow_links_and_cancels() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("data"), [0; 12]).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("/", temp.path().join("link")).unwrap();
    assert_eq!(
        disk_usage::measure(temp.path(), &AtomicBool::new(false))
            .unwrap()
            .0,
        12
    );
    assert!(disk_usage::measure(temp.path(), &AtomicBool::new(true)).is_err());
    assert!(discovery::discover(&[temp.path().into()], &AtomicBool::new(true)).is_err());
}
#[test]
fn scan_reports_missing_paths_and_trash_stays_disabled() {
    let temp = tempfile::tempdir().unwrap();
    let app = bundle(temp.path(), "One");
    // macOS /var and /tmp are symlinks; use canonical fixtures for safety tests.
    let root = temp.path().canonicalize().unwrap();
    let report = scanner::scan(&app, &[root.join("missing")], &AtomicBool::new(false)).unwrap();
    assert_eq!(report.issues.len(), 1);
    let plan = Plan {
        application: app,
        bundle_bytes: 0,
        findings: vec![],
        issues: report.issues,
        data_only: false,
        execution_enabled: false,
    };
    assert!(trash::execute(&plan).is_err());
}
#[test]
#[cfg(unix)]
fn ancestor_symlinks_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    fs::create_dir(root.join("real")).unwrap();
    fs::write(root.join("real/file"), "x").unwrap();
    std::os::unix::fs::symlink(root.join("real"), root.join("alias")).unwrap();
    assert!(safety::validate(&root.join("alias/file")).is_err());
}
#[test]
fn cleanup_plan_rejects_protected_app() {
    let app = Application {
        name: "System".into(),
        bundle_id: "com.apple.test".into(),
        path: "/System/Applications/Test.app".into(),
        version: None,
        protected: true,
    };
    let plan = Plan {
        application: app,
        bundle_bytes: 0,
        findings: vec![],
        issues: vec![],
        data_only: false,
        execution_enabled: false,
    };
    assert!(plan.validate(&[]).is_err());
}
#[test]
fn unreadable_bundle_is_reported() {
    let temp = tempfile::tempdir().unwrap();
    let app = bundle(temp.path(), "Denied");
    // A directory in place of Info.plist gives a deterministic read failure,
    // including when the test process can bypass Unix permission bits.
    fs::remove_file(app.path.join("Contents/Info.plist")).unwrap();
    fs::create_dir(app.path.join("Contents/Info.plist")).unwrap();
    let report = discovery::discover(&[temp.path().into()], &AtomicBool::new(false)).unwrap();
    assert!(report.items.is_empty());
    assert_eq!(report.issues.len(), 1);
}

#[test]
fn summary_and_explicit_selection_preserve_shared_data() {
    use rmpkgx::core::{association::Confidence, scanner::Finding};
    let temp = tempfile::tempdir().unwrap();
    let app = bundle(temp.path(), "Example");
    let finding = |shared, confidence, bytes| Finding {
        review_only: false,
        path: temp.path().join(if shared { "shared" } else { "data" }),
        bytes,
        reason: "fixture evidence".into(),
        confidence,
        safety: "review".into(),
        shared,
        selected: false,
    };
    let mut plan = Plan {
        application: app,
        bundle_bytes: 100,
        findings: vec![
            finding(false, Confidence::Medium, 12),
            finding(true, Confidence::High, 30),
        ],
        issues: vec![],
        data_only: false,
        execution_enabled: false,
    };
    assert!(plan.select(0, true).is_ok());
    assert!(plan.select(1, true).is_err());
    assert!(plan.select(99, true).is_err());
    let summary = plan.summary();
    assert_eq!(summary.selected, 1);
    assert_eq!(summary.selected_bytes, 12);
    assert_eq!(summary.remaining, 1);
    assert_eq!(summary.shared, 1);
    plan.select(0, false).unwrap();
    assert_eq!(plan.summary().selected_bytes, 0);
    plan.application.protected = true;
    assert!(plan.select(0, true).is_err());
}

#[test]
fn progress_can_request_cancellation_during_traversal() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("one"), "1").unwrap();
    fs::write(temp.path().join("two"), "22").unwrap();
    let cancel = AtomicBool::new(false);
    let mut seen = vec![];
    let result = disk_usage::measure_with_progress(temp.path(), &cancel, &mut |path| {
        seen.push(path.to_owned());
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    assert!(result.is_err());
    assert_eq!(seen.len(), 1);
}

#[test]
fn launch_agents_are_inspect_only_even_if_high_confidence_or_manually_selected() {
    let temp = tempfile::tempdir().unwrap();
    let app = bundle(temp.path(), "Fixture");
    let mut plan = Plan {
        application: app,
        bundle_bytes: 0,
        data_only: false,
        execution_enabled: true,
        issues: vec![],
        findings: vec![scanner::Finding {
            path: temp.path().join("LaunchAgents/agent.plist"),
            bytes: 0,
            reason: "direct executable".into(),
            confidence: Confidence::High,
            shared: false,
            selected: false,
            safety: "inspect only".into(),
            review_only: true,
        }],
    };
    assert!(plan.select(0, true).is_err());
    assert!(!plan.findings[0].selected);
    plan.data_only = true;
    plan.findings[0].selected = true;
    let error = plan
        .validate(&[temp.path().join("LaunchAgents")])
        .unwrap_err();
    assert!(error.to_string().contains("inspect-only"));
}
