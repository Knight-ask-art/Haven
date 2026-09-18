//! haven-domain: 领域层（实体、值对象、Locator、契约）。
//! 零框架依赖（不依赖 Tauri / rusqlite / reqwest / Windows API）。

pub mod agent;
pub mod comic_catalog;
pub mod comic_identity;
pub mod comic_progress_subject;
pub mod contracts;
pub mod credential;
pub mod entities;
pub mod enums;
pub mod ids;
pub mod locator;
pub mod periodical;
pub mod setting_proposal;
pub mod settings;

pub use agent::*;
pub use comic_catalog::*;
pub use comic_identity::*;
pub use comic_progress_subject::*;
pub use contracts::*;
pub use credential::*;
pub use entities::*;
pub use enums::*;
pub use ids::*;
pub use locator::*;
pub use periodical::*;
pub use setting_proposal::*;
pub use settings::*;
