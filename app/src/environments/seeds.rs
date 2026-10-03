use super::recipe::EnvironmentDraft;

pub(crate) const ALPINE_GIT_V1: &str = "alpine-git-v1";

const SEED_KEY_BYTES: usize = 32;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct SeedKey(String);

#[derive(Clone, Debug)]
pub(crate) struct EnvironmentSeed {
    pub(crate) key: SeedKey,
    pub(crate) draft: EnvironmentDraft,
}

impl SeedKey {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        let key = value.trim();
        if key.is_empty() || key.len() > SEED_KEY_BYTES {
            return None;
        }
        let mut characters = key.chars();
        let first = characters.next()?;
        if !first.is_ascii_alphabetic() {
            return None;
        }
        if !characters.all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        }) {
            return None;
        }
        Some(Self(key.to_owned()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

pub(crate) fn production_seeds() -> Vec<EnvironmentSeed> {
    vec![EnvironmentSeed {
        key: SeedKey::parse(ALPINE_GIT_V1).expect("alpine-git seed key"),
        draft: alpine_git_draft(),
    }]
}

pub(crate) fn alpine_git_draft() -> EnvironmentDraft {
    EnvironmentDraft {
        name: "Alpine Git".to_owned(),
        oci_image: "alpine/git".to_owned(),
        setup_script: "apk add --no-cache ripgrep\n".to_owned(),
    }
}

#[cfg(test)]
mod tests;
