#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ToolId {
    List,
    Read,
    Edit,
    Write,
    Run,
}

impl ToolId {
    pub(crate) const ALL: [Self; 5] = [Self::List, Self::Read, Self::Edit, Self::Write, Self::Run];

    pub(crate) fn parse(name: &str) -> Option<Self> {
        match name {
            "list" => Some(Self::List),
            "read" => Some(Self::Read),
            "edit" => Some(Self::Edit),
            "write" => Some(Self::Write),
            "run" => Some(Self::Run),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Read => "read",
            Self::Edit => "edit",
            Self::Write => "write",
            Self::Run => "run",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::List => "List",
            Self::Read => "Read",
            Self::Edit => "Edit",
            Self::Write => "Write",
            Self::Run => "Run",
        }
    }
}
