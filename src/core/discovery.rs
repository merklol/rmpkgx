use super::{application::Application, Issue, Report};
use crate::error::Error;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

pub fn discover(roots: &[PathBuf], cancel: &AtomicBool) -> Result<Report<Application>, Error> {
    let mut report = Report {
        items: vec![],
        issues: vec![],
    };
    for root in roots {
        let walker = walkdir::WalkDir::new(root)
            .follow_links(false)
            .max_depth(5)
            .into_iter()
            .filter_entry(|e| {
                e.depth() == 0
                    || e.path()
                        .parent()
                        .is_none_or(|p| p.extension().is_none_or(|x| x != "app"))
            });
        for entry in walker {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            match entry {
                Ok(e)
                    if e.file_type().is_dir()
                        && e.path().extension().is_some_and(|x| x == "app") =>
                {
                    match Application::read(e.path()) {
                        Ok(app)
                            if !report
                                .items
                                .iter()
                                .any(|a: &Application| a.path == app.path) =>
                        {
                            report.items.push(app)
                        }
                        Ok(_) => {}
                        Err(err) => report.issues.push(Issue {
                            path: e.path().into(),
                            message: err.to_string(),
                        }),
                    }
                }
                Ok(_) => {}
                Err(e) => report.issues.push(Issue {
                    path: e.path().unwrap_or(root).into(),
                    message: e.to_string(),
                }),
            }
        }
    }
    report
        .items
        .sort_by_key(|a| (a.name.to_lowercase(), a.path.clone()));
    Ok(report)
}

pub fn resolve<'a>(apps: &'a [Application], query: &str) -> anyhow::Result<&'a Application> {
    let found: Vec<_> = apps
        .iter()
        .filter(|a| {
            a.name.eq_ignore_ascii_case(query)
                || a.bundle_id == query
                || a.path.to_string_lossy() == query
        })
        .collect();
    match found.as_slice() {
        [app] => Ok(app),
        [] => anyhow::bail!("no exact name, bundle ID, or absolute path matched {query:?}"),
        _ => anyhow::bail!(
            "ambiguous application; use an absolute path: {}",
            found
                .iter()
                .map(|a| a.path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}
