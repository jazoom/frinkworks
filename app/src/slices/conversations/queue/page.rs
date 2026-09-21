use crate::conversations::{ConversationQueue, QueueDelivery, QueueItemId};

pub(crate) struct QueueView {
    pub(crate) revision: String,
    pub(crate) items: Vec<QueueItemView>,
    pub(crate) follow_up: bool,
    pub(crate) steering: bool,
    pub(crate) return_item: String,
    pub(crate) return_message: &'static str,
}

pub(crate) struct QueueItemView {
    pub(crate) id: String,
    pub(crate) text: String,
    pub(crate) delivery: &'static str,
    pub(crate) confirm_replace: bool,
}

impl QueueView {
    pub(crate) fn from_queue(queue: &ConversationQueue, follow_up: bool, steering: bool) -> Self {
        Self {
            revision: queue.revision.to_string(),
            items: queue
                .items
                .iter()
                .map(|item| QueueItemView {
                    id: item.id.as_hex(),
                    text: item.text.clone(),
                    delivery: match item.delivery {
                        QueueDelivery::FollowUp => "Follow-up",
                        QueueDelivery::Steering => "Steering",
                    },
                    confirm_replace: false,
                })
                .collect(),
            follow_up,
            steering,
            return_item: String::new(),
            return_message: "",
        }
    }

    pub(crate) fn with_return(mut self, item: QueueItemId, message: &'static str) -> Self {
        let id = item.as_hex();
        for queued in &mut self.items {
            queued.confirm_replace = queued.id == id;
        }
        self.return_item = id;
        self.return_message = message;
        self
    }
}
