use crate::model::Value;
use crate::sources::{AsyncSource, CandidateReceiver};
use crate::state::LauncherState;

const CANDIDATE_BATCH_BUFFER: usize = 128;

pub struct Governor {
    state: LauncherState,
    candidate_receiver: CandidateReceiver,
    source_tasks: Vec<tokio::task::JoinHandle<()>>,
}

pub fn run() {
    println!("fzlaunch scaffold");
}

impl Governor {
    pub fn start(sources: impl IntoIterator<Item = Box<dyn AsyncSource>>) -> Self {
        let (sender, candidate_receiver) = tokio::sync::mpsc::channel(CANDIDATE_BATCH_BUFFER);
        let source_tasks = sources
            .into_iter()
            .map(|source| source.stream_candidates(sender.clone()))
            .collect();

        drop(sender);

        Self {
            state: LauncherState::default(),
            candidate_receiver,
            source_tasks,
        }
    }

    pub fn update_input(&mut self, value: Value) {
        self.state.update_input(value);
    }

    pub fn selected(&self) -> Option<Value> {
        self.state.selected()
    }

    pub async fn receive_candidates(&mut self) -> bool {
        let Some(candidates) = self.candidate_receiver.recv().await else {
            return false;
        };

        self.state.feed(candidates);
        true
    }
}

impl Drop for Governor {
    fn drop(&mut self) {
        for task in &self.source_tasks {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::task::JoinHandle;
    use tokio::time;

    use super::*;
    use crate::model::Candidate;
    use crate::sources::CandidateSender;

    struct MockSource {
        interval: Duration,
    }

    impl MockSource {
        fn new(interval: Duration) -> Self {
            Self { interval }
        }
    }

    impl AsyncSource for MockSource {
        fn stream_candidates(self: Box<Self>, sender: CandidateSender) -> JoinHandle<()> {
            tokio::spawn(async move {
                let mut counter = 0;

                loop {
                    time::sleep(self.interval).await;

                    let candidate = Candidate::new(Value::raw(format!("{counter} 00")), 'm', None);
                    if sender.send(vec![candidate]).await.is_err() {
                        break;
                    }

                    counter += 1;
                }
            })
        }
    }

    async fn receive_next_candidate(governor: &mut Governor) {
        time::advance(Duration::from_millis(100)).await;
        assert!(governor.receive_candidates().await);
    }

    #[tokio::test(start_paused = true)]
    async fn governor_updates_ranking_as_input_and_candidates_arrive() {
        let sources =
            vec![Box::new(MockSource::new(Duration::from_millis(100))) as Box<dyn AsyncSource>];
        let mut governor = Governor::start(sources);

        governor.update_input(Value::raw(";m 10"));
        receive_next_candidate(&mut governor).await;
        assert_eq!(governor.selected(), None);

        receive_next_candidate(&mut governor).await;
        assert_eq!(governor.selected(), Some(Value::raw("1 00")));

        governor.update_input(Value::raw(";m 50"));
        assert_eq!(governor.selected(), None);

        for _ in 2..=5 {
            receive_next_candidate(&mut governor).await;
        }

        assert_eq!(governor.selected(), Some(Value::raw("5 00")));

        governor.update_input(Value::raw(";m 10"));
        assert_eq!(governor.selected(), Some(Value::raw("1 00")));

        for _ in 6..=10 {
            receive_next_candidate(&mut governor).await;
        }

        assert_eq!(governor.selected(), Some(Value::raw("10 00")));
    }
}
