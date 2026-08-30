//! haven-application: 应用层（Use Case / 服务编排 / Wire DTO / Projection 映射）。

pub mod mapper;
pub mod services;
pub mod wire;

pub mod prelude {
    pub use haven_common::AppError;
    pub use haven_domain as domain;
}
