//! Semver helpers for dependency resolution.

use semver::{Version, VersionReq};

pub fn strip_v_prefix(tag: &str) -> &str {
    tag.strip_prefix('v')
        .or_else(|| tag.strip_prefix('V'))
        .unwrap_or(tag)
}

pub fn parse_req(s: &str) -> Result<VersionReq, semver::Error> {
    VersionReq::parse(s)
}

pub fn parse_version(s: &str) -> Result<Version, semver::Error> {
    Version::parse(s)
}

/// Pick the highest semver among `(version, tag, commit)` candidates.
pub fn highest_matching(
    candidates: &[(Version, String, String)],
) -> Option<(String, String, String)> {
    candidates
        .iter()
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(v, tag, commit)| (v.to_string(), tag.clone(), commit.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_highest_matching_version() {
        let req = parse_req("^1.0").unwrap();
        let all = [
            (parse_version("1.0.0").unwrap(), "v1.0.0".into(), "aaa".into()),
            (parse_version("1.2.0").unwrap(), "v1.2.0".into(), "bbb".into()),
            (parse_version("2.0.0").unwrap(), "v2.0.0".into(), "ccc".into()),
        ];
        let matched: Vec<_> = all
            .into_iter()
            .filter(|(v, _, _)| req.matches(v))
            .collect();
        let best = highest_matching(&matched).unwrap();
        assert_eq!(best.0, "1.2.0");
        assert_eq!(best.2, "bbb");
    }
}
