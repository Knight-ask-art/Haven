import { renderToStaticMarkup } from "react-dom/server"
import { MemoryRouter } from "react-router"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { HomePage } from "./HomePage"

function stubLocalStorage(): void {
  const store = new Map<string, string>()
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => store.get(key) ?? null,
    setItem: (key: string, value: string) => {
      store.set(key, value)
    },
    removeItem: (key: string) => {
      store.delete(key)
    },
    key: (index: number) => [...store.keys()][index] ?? null,
    get length() {
      return store.size
    },
  })
}

describe("HomePage 页脚徽标诚实性（死入口治理）", () => {
  beforeEach(stubLocalStorage)
  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it("不渲染任何无动作的占位导航链接", () => {
    const html = renderToStaticMarkup(
      <MemoryRouter initialEntries={["/"]}>
        <HomePage />
      </MemoryRouter>
    )

    expect(html).not.toContain('href="#language"')
    expect(html).not.toContain('href="/rss.xml"')
    expect(html).not.toContain("https://github.com/")
    // 无动作的占位文案同样不允许留在页脚：它们是同一类死入口的另一种写法，
    // 只是把「点不动」换成了「写着一句做不到的话」。
    expect(html).not.toContain("多语言界面将在后续版本提供")
    expect(html).not.toContain("RSS 订阅将在后续版本提供")
    expect(html).not.toContain("GitHub 仓库地址待配置")
  })

  it("保留业务一级入口与真实路由", () => {
    const html = renderToStaticMarkup(
      <MemoryRouter initialEntries={["/"]}>
        <HomePage />
      </MemoryRouter>
    )

    expect(html).toContain('href="/library"')
    expect(html).toContain('href="/search"')
    expect(html).toContain('href="/footprints"')
    expect(html).toContain('href="/downloads"')
  })
})
