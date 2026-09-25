use crate::execution::resources::{MAXIMUM_SKILL_BODY_BYTES, content_hash, parse_skill_metadata};

pub(crate) const SKILL_FILE_NAME: &str = "SKILL.md";

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SkillRecord {
    pub(crate) directory: String,
    pub(crate) fingerprint: String,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) markdown: String,
}

impl SkillRecord {
    pub(super) fn parse(directory: String, markdown: String) -> Result<Self, SkillError> {
        if markdown.len() > MAXIMUM_SKILL_BODY_BYTES {
            return Err(SkillError::Bound);
        }
        let metadata =
            parse_skill_metadata(markdown.as_bytes()).map_err(|_| SkillError::Metadata)?;
        Ok(Self {
            directory,
            fingerprint: content_hash(markdown.as_bytes()),
            name: metadata.name,
            description: metadata.description,
            markdown,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SkillError {
    Persist,
    Unsafe,
    Full,
    Missing,
    Conflict,
    Bound,
    Metadata,
    Directory,
}

impl SkillError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Persist => {
                "Frinkworks cannot read or store a skill file. Make sure that the global directory is accessible."
            }
            Self::Unsafe => {
                "A skill path is not a regular file or directory. Symbolic links are not allowed."
            }
            Self::Full => "The skill directory exceeds the limit of 64 entries.",
            Self::Missing => "That skill file does not exist.",
            Self::Conflict => {
                "That skill file changed or its folder already exists. Reload before you save or delete it."
            }
            Self::Bound => "The complete SKILL.md file must not exceed 1 MiB.",
            Self::Metadata => {
                "Use valid YAML frontmatter with a name and description. The header must not exceed 16 KiB. The name and description limits are 256 and 4096 bytes."
            }
            Self::Directory => {
                "Use a folder name of at most 128 letters, digits, hyphens, underscores or dots. The names . and .. are not allowed."
            }
        }
    }
}
