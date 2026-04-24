// Re-export the generated tonic / prost types.
pub mod remote_desktop {
    tonic::include_proto!("remote_desktop");
}

pub use remote_desktop::*;
