use super::definition::{ArtefactKind, OutputKind};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SystemCommandId {
    RepositoryStatus,
}

impl SystemCommandId {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "repository-status" => Some(Self::RepositoryStatus),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        "repository-status"
    }

    pub(crate) fn label(self) -> &'static str {
        "Repository status"
    }

    pub(crate) fn consequence(self) -> &'static str {
        "Reads the live repository status."
    }

    pub(crate) fn contract(self) -> SystemCommandContract {
        SystemCommandContract {
            required_inputs: &[],
            required_outputs: &[],
        }
    }

    pub(crate) fn all() -> [Self; 1] {
        [Self::RepositoryStatus]
    }
}

pub(crate) struct SystemCommandContract {
    pub(crate) required_inputs: &'static [ArtefactKind],
    pub(crate) required_outputs: &'static [OutputKind],
}

impl SystemCommandContract {
    pub(crate) fn accepts(&self, inputs: &[ArtefactKind], outputs: &[OutputKind]) -> bool {
        kinds_match(inputs, self.required_inputs) && kinds_match(outputs, self.required_outputs)
    }
}

pub(crate) fn kinds_match<T: Copy + Eq>(declared: &[T], required: &[T]) -> bool {
    if declared.len() != required.len() {
        return false;
    }
    let mut remaining = required.to_vec();
    for item in declared {
        let Some(index) = remaining.iter().position(|required| required == item) else {
            return false;
        };
        remaining.remove(index);
    }
    remaining.is_empty()
}

#[cfg(test)]
mod tests;
