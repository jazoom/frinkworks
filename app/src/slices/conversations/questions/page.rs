use crate::conversations::PendingQuestion;

pub(crate) struct QuestionView {
    pub(crate) job: String,
    pub(crate) tool_call: String,
    pub(crate) prompt: String,
    pub(crate) options: Vec<QuestionOptionView>,
    pub(crate) allow_free_text: bool,
}

pub(crate) struct QuestionOptionView {
    pub(crate) id: String,
    pub(crate) label: String,
}

impl QuestionView {
    pub(crate) fn from_question(question: PendingQuestion) -> Self {
        Self {
            job: question.job.as_hex(),
            tool_call: question.tool_call,
            prompt: question.prompt,
            options: question
                .options
                .into_iter()
                .map(|option| QuestionOptionView {
                    id: option.id,
                    label: option.label,
                })
                .collect(),
            allow_free_text: question.allow_free_text,
        }
    }
}
