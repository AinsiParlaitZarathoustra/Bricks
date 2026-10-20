//! Backend implementations of `SandboxRuntime`.

pub mod local;

#[cfg(feature = "backend-docker")]
pub mod docker;

pub use local::LocalProcessRuntime;

#[cfg(feature = "backend-docker")]
pub use docker::DockerRuntime;
