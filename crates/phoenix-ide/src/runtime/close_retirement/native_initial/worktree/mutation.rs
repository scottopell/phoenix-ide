//! Original-context admission for individual owned filesystem mutations.
//!
//! This executor is used only inside an independently retained physical worker.
//! Unknown writer release parks that worker stack (including descriptor and
//! repository custody); it never becomes an ordinary traversal error or retry.

use super::super::super::WorktreeMutationExecutor;
use super::{
    NativeCloseInitialEntryRelease, NativeCloseInitialFirstEffectError,
    OriginalWorktreeEffectContext, WorkerEntryResult,
};

pub(super) struct OriginalWorktreeMutationExecutor<'a> {
    original: &'a mut OriginalWorktreeEffectContext,
    runtime: &'a tokio::runtime::Handle,
    refusal: Option<String>,
}

impl<'a> OriginalWorktreeMutationExecutor<'a> {
    pub(super) fn new(
        original: &'a mut OriginalWorktreeEffectContext,
        runtime: &'a tokio::runtime::Handle,
    ) -> Self {
        Self {
            original,
            runtime,
            refusal: None,
        }
    }

    fn refuse<T>(&mut self, detail: String) -> Result<T, String> {
        self.refusal = Some(detail.clone());
        Err(detail)
    }

    fn retain_unknown<T>(&self, entry: T) -> ! {
        self.original
            .fatal
            .close("native_close_initial_worktree_mutation_unknown_writer_release");
        // The outer retained worker also owns the original repository lock and
        // every descriptor traversed so far. Keeping this stack live prevents
        // release of that custody and of any unclassified callback result.
        let _retained_entry = entry;
        loop {
            std::thread::park();
        }
    }
}

impl WorktreeMutationExecutor for OriginalWorktreeMutationExecutor<'_> {
    fn execute<T, F>(&mut self, effect: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T, String> + Send + 'static,
    {
        if let Some(refusal) = &self.refusal {
            return Err(refusal.clone());
        }
        let entry = self.original.admitted_leaf(self.runtime, effect);
        let confirmed = matches!(
            &entry,
            Ok((_, NativeCloseInitialEntryRelease::Confirmed))
                | Err(NativeCloseInitialFirstEffectError::NotStarted(_)
                    | NativeCloseInitialFirstEffectError::Rejected {
                        release: NativeCloseInitialEntryRelease::Confirmed,
                        ..
                    },)
        );
        if !confirmed {
            self.retain_unknown(entry);
        }
        self.original.classify_confirmed_release();
        match entry {
            Ok((WorkerEntryResult::Ran(result), NativeCloseInitialEntryRelease::Confirmed)) => {
                // A syscall error is not renewed permission to compensate:
                // any compensating mutation must execute through this same
                // admission boundary again, under the unchanged deadline.
                result
            }
            Ok((
                WorkerEntryResult::Busy | WorkerEntryResult::Refused,
                NativeCloseInitialEntryRelease::Confirmed,
            )) => self.refuse("original Close source unavailable at Worktree mutation".into()),
            Err(
                NativeCloseInitialFirstEffectError::NotStarted(error)
                | NativeCloseInitialFirstEffectError::Rejected {
                    error,
                    release: NativeCloseInitialEntryRelease::Confirmed,
                },
            ) => self.refuse(format!(
                "native Worktree mutation authorization refused: {error}"
            )),
            // Classified above. Keep this branch fail-closed if the release
            // classification is extended, rather than permitting traversal.
            entry => self.retain_unknown(entry),
        }
    }
}
