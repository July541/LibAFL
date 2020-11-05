use crate::inputs::Input;
use crate::observers::Observer;
use crate::AflError;

use crate::executors::{Executor, ExitKind};

use std::os::raw::c_void;
use std::ptr;

type HarnessFunction<I> = fn(&dyn Executor<I>, &[u8]) -> ExitKind;

pub struct InMemoryExecutor<I>
where
    I: Input,
{
    cur_input: Option<Box<I>>,
    observers: Vec<Box<dyn Observer>>,
    harness: HarnessFunction<I>,
}

static mut CURRENT_INMEMORY_EXECUTOR_PTR: *const c_void = ptr::null();

impl<I> Executor<I> for InMemoryExecutor<I>
where
    I: Input,
{
    fn run_target(&mut self) -> Result<ExitKind, AflError> {
        let bytes = match self.cur_input.as_ref() {
            Some(i) => i.serialize(),
            None => return Err(AflError::Empty("cur_input".to_string())),
        };
        unsafe {
            CURRENT_INMEMORY_EXECUTOR_PTR = self as *const InMemoryExecutor<I> as *const c_void;
        }
        let ret = match bytes {
            Ok(b) => Ok((self.harness)(self, b)),
            Err(e) => Err(e),
        };
        unsafe {
            CURRENT_INMEMORY_EXECUTOR_PTR = ptr::null();
        }
        ret
    }

    fn place_input(&mut self, input: Box<I>) -> Result<(), AflError> {
        self.cur_input = Some(input);
        Ok(())
    }

    fn cur_input(&self) -> &Option<Box<I>> {
        &self.cur_input
    }

    fn cur_input_mut(&mut self) -> &mut Option<Box<I>> {
        &mut self.cur_input
    }

    fn reset_observers(&mut self) -> Result<(), AflError> {
        for observer in &mut self.observers {
            observer.reset()?;
        }
        Ok(())
    }

    fn post_exec_observers(&mut self) -> Result<(), AflError> {
        self.observers
            .iter_mut()
            .map(|x| x.post_exec())
            .fold(Ok(()), |acc, x| if x.is_err() { x } else { acc })
    }

    fn add_observer(&mut self, observer: Box<dyn Observer>) {
        self.observers.push(observer);
    }

    fn observers(&self) -> &Vec<Box<dyn Observer>> {
        &self.observers
    }
}

impl<I> InMemoryExecutor<I>
where
    I: Input,
{
    pub fn new(harness_fn: HarnessFunction<I>) -> Self {
        unsafe {
            os_signals::setup_crash_handlers::<I, Self>();
        }
        InMemoryExecutor {
            cur_input: None,
            observers: vec![],
            harness: harness_fn,
        }
    }
}

#[cfg(unix)]
pub mod unix_signals {

    extern crate libc;
    use self::libc::{c_int, c_void, sigaction, siginfo_t};
    // Unhandled signals: SIGALRM, SIGHUP, SIGINT, SIGKILL, SIGQUIT, SIGTERM
    use self::libc::{
        SA_NODEFER, SA_SIGINFO, SIGABRT, SIGBUS, SIGFPE, SIGILL, SIGPIPE, SIGSEGV, SIGUSR2,
    };
    use std::io::{stdout, Write}; // Write brings flush() into scope
    use std::{mem, process, ptr};

    use crate::executors::inmemory::CURRENT_INMEMORY_EXECUTOR_PTR;
    use crate::executors::Executor;
    use crate::inputs::Input;

    pub extern "C" fn libaflrs_executor_inmem_handle_crash<I, E>(
        _sig: c_int,
        info: siginfo_t,
        _void: c_void,
    ) where
        I: Input,
        E: Executor<I>,
    {
        unsafe {
            if CURRENT_INMEMORY_EXECUTOR_PTR == ptr::null() {
                println!(
                    "We died accessing addr {}, but are not in client...",
                    info.si_addr() as usize
                );
            }
        }
        // TODO: LLMP
        println!("Child crashed!");
        let _ = stdout().flush();
    }

    pub extern "C" fn libaflrs_executor_inmem_handle_timeout<I, E>(
        _sig: c_int,
        _info: siginfo_t,
        _void: c_void,
    ) where
        I: Input,
        E: Executor<I>,
    {
        dbg!("TIMEOUT/SIGUSR2 received");
        unsafe {
            if CURRENT_INMEMORY_EXECUTOR_PTR == ptr::null() {
                dbg!("TIMEOUT or SIGUSR2 happened, but currently not fuzzing.");
                return;
            }
        }
        // TODO: send LLMP.
        println!("Timeout in fuzz run.");
        let _ = stdout().flush();
        process::abort();
    }

    pub unsafe fn setup_crash_handlers<I, E>()
    where
        I: Input,
        E: Executor<I>,
    {
        let mut sa: sigaction = mem::zeroed();
        libc::sigemptyset(&mut sa.sa_mask as *mut libc::sigset_t);
        sa.sa_flags = SA_NODEFER | SA_SIGINFO;
        sa.sa_sigaction = libaflrs_executor_inmem_handle_crash::<I, E> as usize;
        for (sig, msg) in &[
            (SIGSEGV, "segfault"),
            (SIGBUS, "sigbus"),
            (SIGABRT, "sigabrt"),
            (SIGILL, "illegal instruction"),
            (SIGFPE, "fp exception"),
            (SIGPIPE, "pipe"),
        ] {
            if sigaction(*sig, &mut sa as *mut sigaction, ptr::null_mut()) < 0 {
                panic!("Could not set up {} handler", &msg);
            }
        }

        sa.sa_sigaction = libaflrs_executor_inmem_handle_timeout::<I, E> as usize;
        if sigaction(SIGUSR2, &mut sa as *mut sigaction, ptr::null_mut()) < 0 {
            panic!("Could not set up sigusr2 handler for timeouts");
        }
    }
}

#[cfg(unix)]
use unix_signals as os_signals;
#[cfg(not(unix))]
compile_error!("InMemoryExecutor not yet supported on this OS");

#[cfg(test)]
mod tests {
    use crate::executors::inmemory::InMemoryExecutor;
    use crate::executors::{Executor, ExitKind};
    use crate::inputs::Input;
    use crate::observers::Observer;
    use crate::AflError;
    use std::any::Any;

    #[derive(Clone)]
    struct NopInput {}
    impl Input for NopInput {
        fn serialize(&self) -> Result<&[u8], AflError> {
            Ok("NOP".as_bytes())
        }
        fn deserialize(&mut self, _buf: &[u8]) -> Result<(), AflError> {
            Ok(())
        }
    }

    struct Nopserver {}

    impl Observer for Nopserver {
        fn reset(&mut self) -> Result<(), AflError> {
            Err(AflError::Unknown("Nop reset, testing only".to_string()))
        }
        fn post_exec(&mut self) -> Result<(), AflError> {
            Err(AflError::Unknown("Nop exec, testing only".to_string()))
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    fn test_harness_fn_nop(_executor: &dyn Executor<NopInput>, buf: &[u8]) -> ExitKind {
        println! {"Fake exec with buf of len {}", buf.len()};
        ExitKind::Ok
    }

    #[test]
    fn test_inmem_post_exec() {
        let mut in_mem_executor = InMemoryExecutor::new(test_harness_fn_nop);
        let nopserver = Nopserver {};
        in_mem_executor.add_observer(Box::new(nopserver));
        assert_eq!(in_mem_executor.post_exec_observers().is_err(), true);
    }

    #[test]
    fn test_inmem_exec() {
        let mut in_mem_executor = InMemoryExecutor::new(test_harness_fn_nop);
        let input = NopInput {};
        assert!(in_mem_executor.place_input(Box::new(input)).is_ok());
        assert!(in_mem_executor.run_target().is_ok());
    }
}
