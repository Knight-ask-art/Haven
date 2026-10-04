// jsdom 不实现原生 dialog API。这里只补单元测试的开闭状态，不模拟浏览器的
// top layer / inert；真实模态隔离仍必须在浏览器里单独验证。
if (typeof HTMLDialogElement !== "undefined") {
  if (typeof HTMLDialogElement.prototype.showModal !== "function") {
    Object.defineProperty(HTMLDialogElement.prototype, "showModal", {
      configurable: true,
      value(this: HTMLDialogElement) { this.setAttribute("open", "") },
    })
  }
  if (typeof HTMLDialogElement.prototype.close !== "function") {
    Object.defineProperty(HTMLDialogElement.prototype, "close", {
      configurable: true,
      value(this: HTMLDialogElement) { this.removeAttribute("open") },
    })
  }
}
