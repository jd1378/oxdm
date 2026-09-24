//! Naming downloads on the command line: a full id, or enough of its
//! start to be unambiguous (`list` prints the first eight characters).

use super::failure::Failure;
use crate::domain::{Job, JobId};

/// Shorter prefixes match too much to be a deliberate choice.
const MIN_PREFIX: usize = 4;

pub fn resolve(jobs: &[Job], selector: &str) -> Result<JobId, Failure> {
    let wanted = selector.trim().to_ascii_lowercase();
    if let Ok(id) = wanted.parse::<JobId>() {
        return jobs
            .iter()
            .any(|j| j.id == id)
            .then_some(id)
            .ok_or_else(|| Failure::usage(format!("no download with id {selector}")));
    }
    if wanted.len() < MIN_PREFIX {
        return Err(Failure::usage(format!(
            "{selector}: give at least {MIN_PREFIX} characters of a download id"
        )));
    }
    let hits: Vec<&Job> = jobs
        .iter()
        .filter(|j| j.id.to_string().starts_with(&wanted))
        .collect();
    match hits.as_slice() {
        [one] => Ok(one.id),
        [] => Err(Failure::usage(format!("no download with id {selector}"))),
        many => Err(Failure::usage(format!(
            "{selector} matches {} downloads; give more of the id ({})",
            many.len(),
            many.iter()
                .map(|j| j.id.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// Every selector resolved, in order, each id once.
pub fn resolve_all(jobs: &[Job], selectors: &[String]) -> Result<Vec<JobId>, Failure> {
    let mut out = Vec::with_capacity(selectors.len());
    for s in selectors {
        let id = resolve(jobs, s)?;
        if !out.contains(&id) {
            out.push(id);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job_with(id: &str) -> Job {
        let mut j = crate::cli::fixtures::job();
        j.id = id.parse().unwrap();
        j
    }

    #[test]
    fn a_full_id_or_a_unique_prefix_names_a_download() {
        let a = "a1b2c3d4-0000-4000-8000-000000000001";
        let b = "a1b2ffff-0000-4000-8000-000000000002";
        let jobs = [job_with(a), job_with(b)];
        assert_eq!(resolve(&jobs, a).unwrap().to_string(), a);
        assert_eq!(resolve(&jobs, "A1B2C3").unwrap().to_string(), a);
        assert!(
            resolve(&jobs, "a1b2")
                .unwrap_err()
                .message
                .contains("matches 2")
        );
        assert!(resolve(&jobs, "a1b").is_err(), "too short to be deliberate");
        assert!(resolve(&jobs, "ffff0000").is_err());
        assert!(
            resolve(&jobs, "a1b2c3d4-0000-4000-8000-00000000000f").is_err(),
            "a well-formed id that is not in the list"
        );
    }

    #[test]
    fn repeats_collapse() {
        let a = "a1b2c3d4-0000-4000-8000-000000000001";
        let jobs = [job_with(a)];
        let ids = resolve_all(&jobs, &[a.into(), "a1b2c3d4".into()]).unwrap();
        assert_eq!(ids.len(), 1);
    }
}
