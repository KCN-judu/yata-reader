//! The two ways a session reaches memory: the desktop channel over the game process, and a
//! synthetic image for tests and fixtures. Both attach by discovering the runtime's layout and
//! read through the same [`crate::layout`] code.

use yata_protocol::probe::{Channel, ReadResult, Scope, TargetProcess, code, exit};

use crate::layout::cpython::{LayoutError, Runtime, discover};
use crate::layout::memory::{Cached, Image, Memory};
use crate::layout::souls::{Cancelled, read_souls};
use crate::session::{Attached, Backend, Failure};

/// Memory with its runtime found.
struct Attachment<M> {
    memory: Cached<M>,
    runtime: Runtime,
}

fn layout_failure(e: &LayoutError) -> Failure {
    Failure::new(
        code::LAYOUT_MISMATCH,
        format!("the game's memory is not a known layout: {e:?}"),
        exit::NO_STRATEGY,
    )
}

fn attach_to<M: Memory>(memory: M) -> Result<Attachment<M>, Failure> {
    let memory = Cached::new(memory);
    let runtime = discover(&memory, &|| false).map_err(|e| layout_failure(&e))?;
    Ok(Attachment { memory, runtime })
}

fn read_from<M: Memory>(
    a: Option<&Attachment<M>>,
    scope: Scope,
    progress: &mut dyn FnMut(u64, u64),
    cancelled: &dyn Fn() -> bool,
) -> Result<ReadResult, Failure> {
    let a = a.ok_or_else(|| Failure::new(code::NOT_ATTACHED, "no game is attached", 0))?;
    match scope {
        Scope::Souls => read_souls(&a.memory, &a.runtime, progress, cancelled)
            .map_err(|Cancelled| Failure::new(code::CANCELLED, "cancelled", 0)),
        Scope::Unspecified => Err(Failure::new(
            code::SCOPE_UNSUPPORTED,
            "this reader does not read that scope",
            0,
        )),
    }
}

/// Synthetic memory standing in for a game process.
pub struct ImageBackend {
    image: Option<Image>,
    target: TargetProcess,
    attached: Option<Attachment<Image>>,
}

impl ImageBackend {
    pub fn new(image: Image, target: TargetProcess) -> ImageBackend {
        ImageBackend {
            image: Some(image),
            target,
            attached: None,
        }
    }
}

impl Backend for ImageBackend {
    fn attach(&mut self, target_pid: u32) -> Result<Attached, Failure> {
        if target_pid != 0 && target_pid != self.target.pid {
            return Err(Failure::new(
                code::NOT_FOUND,
                format!("no process has pid {target_pid}"),
                exit::NOT_ATTACHED,
            ));
        }
        let image = self
            .image
            .take()
            .ok_or_else(|| Failure::new(code::INTERNAL, "attached twice", exit::INTERNAL))?;
        let a = attach_to(image)?;
        let engine = a.runtime.engine().to_owned();
        self.attached = Some(a);
        Ok(Attached {
            engine,
            channel: Channel::DesktopMemory,
            target: self.target.clone(),
        })
    }

    fn read(
        &mut self,
        scope: Scope,
        progress: &mut dyn FnMut(u64, u64),
        cancelled: &dyn Fn() -> bool,
    ) -> Result<ReadResult, Failure> {
        read_from(self.attached.as_ref(), scope, progress, cancelled)
    }
}

/// The desktop channel: the game process on this machine.
#[derive(Default)]
pub struct DesktopBackend {
    #[cfg(windows)]
    attached: Option<Attachment<crate::desktop::GameMemory>>,
}

impl Backend for DesktopBackend {
    #[cfg(windows)]
    fn attach(&mut self, target_pid: u32) -> Result<Attached, Failure> {
        use crate::desktop::{self, AttachError};
        let (chosen, memory) = desktop::attach(target_pid).map_err(|e| {
            let mut f = Failure::new(e.code(), e.message(), e.exit());
            f.os_error = e.os_error();
            if let AttachError::Ambiguous { candidates } = &e {
                f.candidates = candidates.iter().map(desktop::target_of).collect();
            }
            f
        })?;
        let a = attach_to(memory)?;
        let engine = a.runtime.engine().to_owned();
        self.attached = Some(a);
        Ok(Attached {
            engine,
            channel: Channel::DesktopMemory,
            target: desktop::target_of(&chosen),
        })
    }

    #[cfg(not(windows))]
    fn attach(&mut self, _target_pid: u32) -> Result<Attached, Failure> {
        Err(Failure::new(
            code::UNSUPPORTED_ENVIRONMENT,
            "the desktop channel reads the Windows game, and this is not Windows",
            exit::NO_STRATEGY,
        ))
    }

    #[cfg(windows)]
    fn read(
        &mut self,
        scope: Scope,
        progress: &mut dyn FnMut(u64, u64),
        cancelled: &dyn Fn() -> bool,
    ) -> Result<ReadResult, Failure> {
        read_from(self.attached.as_ref(), scope, progress, cancelled)
    }

    #[cfg(not(windows))]
    fn read(
        &mut self,
        _scope: Scope,
        _progress: &mut dyn FnMut(u64, u64),
        _cancelled: &dyn Fn() -> bool,
    ) -> Result<ReadResult, Failure> {
        Err(Failure::new(code::NOT_ATTACHED, "no game is attached", 0))
    }
}
