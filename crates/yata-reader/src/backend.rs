//! The two ways a session reaches memory: the desktop channel over the game process, and a
//! synthetic image for tests and fixtures. Both attach by discovering the runtime's layout and
//! read through the same [`crate::layout`] code.

use yata_protocol::probe::{Channel, Reading, Scope, TargetProcess};

use crate::layout::cpython::{Cancelled, LayoutError, Runtime, discover};
use crate::layout::memory::{Cached, Image, Memory};
use crate::layout::souls::read_souls;
use crate::session::{
    Attached, Backend, RequestFailure, RequestReason, SessionFailure, SessionReason, Target,
};

/// Memory with its runtime found.
struct Attachment<M> {
    memory: Cached<M>,
    runtime: Runtime,
}

fn layout_failure(e: &LayoutError) -> SessionFailure {
    SessionFailure::new(
        SessionReason::LayoutMismatch,
        format!("the game's memory is not a known layout: {e:?}"),
    )
}

fn attach_to<M: Memory>(memory: M) -> Result<Attachment<M>, SessionFailure> {
    let memory = Cached::new(memory);
    let runtime = discover(&memory).map_err(|e| layout_failure(&e))?;
    Ok(Attachment { memory, runtime })
}

fn not_attached() -> RequestFailure {
    RequestFailure::new(RequestReason::NotAttached, "no game is attached")
}

fn read_from<M: Memory>(
    a: &Attachment<M>,
    scope: Scope,
    progress: &mut dyn FnMut(u64, u64),
    cancelled: &dyn Fn() -> bool,
) -> Result<Reading, RequestFailure> {
    match scope {
        Scope::Souls => read_souls(&a.memory, &a.runtime, progress, cancelled)
            .map_err(|Cancelled| RequestFailure::new(RequestReason::Cancelled, "cancelled")),
        Scope::Unspecified => Err(RequestFailure::new(
            RequestReason::ScopeUnsupported,
            "this reader does not read that scope",
        )),
    }
}

/// Where a synthetic backend stands.
enum ImageState {
    /// Not attached yet: the memory waits.
    Detached(Image),
    Attached(Attachment<Image>),
    /// An attach was tried and failed; the memory is gone with it.
    Failed,
}

/// Synthetic memory standing in for a game process.
pub struct ImageBackend {
    target: TargetProcess,
    state: ImageState,
}

impl ImageBackend {
    pub fn new(image: Image, target: TargetProcess) -> ImageBackend {
        ImageBackend {
            target,
            state: ImageState::Detached(image),
        }
    }
}

impl Backend for ImageBackend {
    fn attach(&mut self, target: Target) -> Result<Attached, SessionFailure> {
        if let Target::Pid(pid) = target
            && pid.get() != self.target.pid
        {
            return Err(SessionFailure::new(
                SessionReason::NotFound,
                format!("no process has pid {pid}"),
            ));
        }
        let image = match std::mem::replace(&mut self.state, ImageState::Failed) {
            ImageState::Detached(image) => image,
            other => {
                self.state = other;
                return Err(SessionFailure::new(
                    SessionReason::Internal,
                    "attached twice",
                ));
            }
        };
        let a = attach_to(image)?;
        let engine = a.runtime.engine().to_owned();
        self.state = ImageState::Attached(a);
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
    ) -> Result<Reading, RequestFailure> {
        match &self.state {
            ImageState::Attached(a) => read_from(a, scope, progress, cancelled),
            ImageState::Detached(_) | ImageState::Failed => Err(not_attached()),
        }
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
    fn attach(&mut self, target: Target) -> Result<Attached, SessionFailure> {
        use crate::desktop;
        let (chosen, memory) = desktop::attach(target).map_err(|e| e.failure())?;
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
    fn attach(&mut self, _target: Target) -> Result<Attached, SessionFailure> {
        Err(SessionFailure::new(
            SessionReason::UnsupportedEnvironment,
            "the desktop channel reads the Windows game, and this is not Windows",
        ))
    }

    #[cfg(windows)]
    fn read(
        &mut self,
        scope: Scope,
        progress: &mut dyn FnMut(u64, u64),
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Reading, RequestFailure> {
        let a = self.attached.as_ref().ok_or_else(not_attached)?;
        read_from(a, scope, progress, cancelled)
    }

    #[cfg(not(windows))]
    fn read(
        &mut self,
        _scope: Scope,
        _progress: &mut dyn FnMut(u64, u64),
        _cancelled: &dyn Fn() -> bool,
    ) -> Result<Reading, RequestFailure> {
        Err(not_attached())
    }
}
