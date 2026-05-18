// ui.rs - Win32 main window module split into focused source shards.
// Keep these includes ordered: later files rely on shared types/constants from common.rs.

include!("ui/common.rs");
include!("ui/entry.rs");
include!("ui/paint.rs");
include!("ui/create.rs");
include!("ui/draw.rs");
include!("ui/commands.rs");
include!("ui/messages.rs");
include!("ui/utils.rs");
include!("ui/pair_qr.rs");
include!("ui/startup.rs");
