pub(crate) struct CompactionView {
    pub(crate) covered: String,
}

impl CompactionView {
    pub(crate) fn from_record(record: &crate::conversations::CompactionRecord) -> Self {
        Self {
            covered: record.covered_through.as_hex(),
        }
    }
}
