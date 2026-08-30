// SPIKE-FOLIATE-001：vendored foliate-js 为无类型 ES 模块（MIT，见目录内 LICENSE）。
// 通配 ambient 声明使 tsc 通过且无需开启 allowJs——vendored 实现文件不进入类型检查。
declare module "*foliate-js/view.js"

// Spike 构建门控：仅当以 VITE_SPIKE_ENABLED=1 构建时才包含诊断路由。
interface ImportMetaEnv {
  readonly VITE_SPIKE_ENABLED?: string
}
