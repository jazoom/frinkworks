pub(crate) struct CompactionView {
    pub(crate) covered: String,
    pub(crate) request_count: usize,
    pub(crate) preserve: Option<String>,
}

impl CompactionView {
    pub(crate) fn from_record(record: &crate::conversations::CompactionRecord) -> Self {
        Self {
            covered: record.covered_through.as_hex(),
            request_count: record.requests.len(),
            preserve: record.preserve.clone(),
        }
    }
}
