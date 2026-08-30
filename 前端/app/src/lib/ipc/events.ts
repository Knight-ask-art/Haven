// IPC 事件订阅 helper（IPC-FE-001 / SLICE-SCAN-001）。
// transport 名与逻辑契约名不同：Tauri 2 拒绝带点事件名（IllegalEventName），
// 后端已把 `favorite.changed` 适配为 `favorite-changed`、`library.changed` 适配为
// `library-changed`（src-tauri/src/ipc/mod.rs），前端监听必须使用连字符名；
// 事件负载形状以冻结契约为准（§10.2.1）。

import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type { FavoriteChangedDto, LibraryChangedDto } from "./generated/wire";

/** 订阅 favorite-changed（仅状态实际变化时发布；revision 与 FavoriteSetResult 同源）。 */
export function onFavoriteChanged(
  handler: (payload: FavoriteChangedDto) => void,
): Promise<UnlistenFn> {
  return listen<FavoriteChangedDto>("favorite-changed", (event) => handler(event.payload));
}

/** 订阅 library-changed（扫描终态后发布一次；revision=null 表示全量刷新）。 */
export function onLibraryChanged(
  handler: (payload: LibraryChangedDto) => void,
): Promise<UnlistenFn> {
  return listen<LibraryChangedDto>("library-changed", (event) => handler(event.payload));
}
