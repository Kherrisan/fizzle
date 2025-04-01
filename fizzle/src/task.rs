use crate::{
    handlers::{
        process::{ExecTask, ForkHandlerTask, ForkTask},
        signal::SendSignalTask,
        thread::{CancelThreadTask, CreateThreadTask},
    },
    scheduler::{
        CreateCowTask, CreatePrimaryCowTask, FizzleSingleton, HandleExpiredTimerTask,
        HandleLocalSignalTask, HandleProcessSignalTask, HandleThreadSignalTask,
        MoveToCowOriginTask, MoveToPrimaryTask, RecvCowAtOriginTask, ReturnTask, RunSubprocessTask,
        TerminateProcessTask, TerminateThreadTask, TransportCowTask,
    },
};

// This can't be a boxed closure, nor can it be anything that uses dynamic dispatch.
// Otherwise, it will segfault the moment a task is created in one process and executed
// in another. Anything that stores the location of code to run at runtime (function pointers,
// closures, or even fat pointers from `dyn Trait` objects) will ultimately fail, as pointers
// aren't guaranteed to point to the same functions across processes. Even if the processes
// run the same code, ASLR ensures that offsets to library functions will likely be different,
// so runtime-assigned function pointers or vtable pointers are doomed to be unsound.
//
// This assigns the function pointer location at compile time, so the function will be resolved
// at the time the task is popped off the queue and `execute()` is called regardless of which
// process pushed it onto the queue.
pub enum Task {
    CreateCow(CreateCowTask),
    CreatePrimaryCow(CreatePrimaryCowTask),
    TransportCow(TransportCowTask),
    MoveToCowOrigin(MoveToCowOriginTask),
    RecvCowAtOrigin(RecvCowAtOriginTask),

    CancelThread(CancelThreadTask),
    CreateThread(CreateThreadTask),
    Exec(ExecTask),
    Fork(ForkTask),
    ForkHandler(ForkHandlerTask),
    HandleExpiredTimer(HandleExpiredTimerTask),
    HandleLocalSignal(HandleLocalSignalTask),
    HandleProcessSignal(HandleProcessSignalTask),
    HandleThreadSignal(HandleThreadSignalTask),
    MoveToPrimary(MoveToPrimaryTask),
    Return(ReturnTask),
    RunSubprocess(RunSubprocessTask),
    SendSignal(SendSignalTask),
    TerminateProcess(TerminateProcessTask),
    TerminateThread(TerminateThreadTask),
}

impl Task {
    pub fn execute(self, ctx: &mut FizzleSingleton) -> TaskResult {
        match self {
            Self::CreateCow(t) => t.execute(ctx),
            Self::CreatePrimaryCow(t) => t.execute(ctx),
            Self::TransportCow(t) => t.execute(ctx),
            Self::MoveToCowOrigin(t) => t.execute(ctx),
            Self::RecvCowAtOrigin(t) => t.execute(ctx),
            Self::CancelThread(t) => t.execute(ctx),
            Self::CreateThread(t) => t.execute(ctx),
            Self::Exec(t) => t.execute(ctx),
            Self::Fork(t) => t.execute(ctx),
            Self::ForkHandler(t) => t.execute(ctx),
            Self::HandleExpiredTimer(t) => t.execute(ctx),
            Self::HandleLocalSignal(t) => t.execute(ctx),
            Self::HandleProcessSignal(t) => t.execute(ctx),
            Self::HandleThreadSignal(t) => t.execute(ctx),
            Self::MoveToPrimary(t) => t.execute(ctx),
            Self::Return(t) => t.execute(ctx),
            Self::RunSubprocess(t) => t.execute(ctx),
            Self::SendSignal(t) => t.execute(ctx),
            Self::TerminateProcess(t) => t.execute(ctx),
            Self::TerminateThread(t) => t.execute(ctx),
        }
    }
}

pub enum TaskResult {
    Continue,
    Suspend,
    Return,
}
