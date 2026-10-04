import { useEffect, useRef, useState } from "react"
import { BootstrapIcon } from "@/components/ui/haven/BootstrapIcon"
import { SETTINGS_NAV_GROUPS, SETTINGS_OVERVIEW, SETTINGS_REGISTRY, type SettingsSectionId } from "../lib/settings-registry"
import { filterSettingsNavigationGroups, findSettingsFeatureMatches, settingsSectionSearchKeywords, type SettingsFeatureSearchMatch } from "../lib/settings-navigation"
import "./settings-navigation.css"

export type SettingsNavId = SettingsSectionId | "overview"

const groups = SETTINGS_NAV_GROUPS.map((group) => ({
  label: group.label,
  items: [
    ...(group.id === "configure" ? [SETTINGS_OVERVIEW] : []),
    ...group.sectionIds.flatMap((id) => {
      const entry = SETTINGS_REGISTRY.find((item) => item.id === id)
      return entry ? [{ ...entry, keywords: settingsSectionSearchKeywords(entry.id) }] : []
    }),
  ],
}))

/** 导航、搜索与窄窗口呈现只有一个 owner，不持有设置值或调用 IPC。 */
export function SettingsNavigation({ activeId, onSelectSection, onSelectFeature }: {
  activeId: SettingsNavId
  onSelectSection: (id: SettingsNavId) => void
  onSelectFeature: (match: SettingsFeatureSearchMatch) => void
}) {
  const [query, setQuery] = useState("")
  const inputRef = useRef<HTMLInputElement>(null)
  const normalized = query.trim()
  const visibleGroups = filterSettingsNavigationGroups(groups, normalized)
  const matches = findSettingsFeatureMatches(normalized)
  const shortcut = /Mac|iPhone|iPad/.test(navigator.userAgent) ? "⌘ K" : "Ctrl K"

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      // 所有模态窗口共享同一条边界，不让设置搜索抢走弹窗或原生对话框的焦点。
      if (event.defaultPrevented || document.querySelector('dialog[open], [aria-modal="true"]')) return
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k") {
        event.preventDefault()
        inputRef.current?.focus()
        inputRef.current?.select()
      } else if (event.key === "Escape" && document.activeElement === inputRef.current) {
        event.preventDefault()
        setQuery("")
        inputRef.current?.blur()
      }
    }
    document.addEventListener("keydown", onKeyDown)
    return () => document.removeEventListener("keydown", onKeyDown)
  }, [])

  const selectSection = (id: SettingsNavId) => {
    setQuery("")
    onSelectSection(id)
  }
  const selectFeature = (match: SettingsFeatureSearchMatch) => {
    // 搜索结果将卸载；先把焦点交还始终存在的搜索框，作为模态关闭后的返回位置。
    inputRef.current?.focus({ preventScroll: true })
    setQuery("")
    onSelectFeature(match)
  }

  return <aside className="settings-navigation">
    <div className="settings-navigation__brand">
      <span><img src="/logo.png" alt="" /></span><strong>栖阅</strong>
    </div>
    <div className="settings-navigation__search">
      <BootstrapIcon name="search" size={16} />
      <input ref={inputRef} value={query} onChange={(event) => setQuery(event.target.value)}
        placeholder="搜索设置" aria-label="搜索设置" aria-keyshortcuts="Control+K Meta+K"
        onKeyDown={(event) => {
          if (event.key !== "Enter") return
          event.preventDefault()
          if (matches[0]) selectFeature(matches[0])
          else {
            const item = visibleGroups.flatMap((group) => group.items)[0]
            if (item) selectSection(item.id)
          }
        }} />
      <kbd>{shortcut}</kbd>
    </div>
    <select className="settings-navigation__compact" aria-label="设置分类选择" value={activeId}
      onChange={(event) => {
        const item = groups.flatMap((group) => group.items).find((item) => item.id === event.target.value)
        if (item) selectSection(item.id)
      }}>
      {groups.map((group) => <optgroup label={group.label} key={group.label}>
        {group.items.map((item) => <option key={item.id} value={item.id}>{item.label}</option>)}
      </optgroup>)}
    </select>
    <nav aria-label={normalized ? "设置搜索结果" : "设置分类"}
      className={`settings-navigation__nav${normalized ? " settings-navigation__nav--search" : ""}`}>
      {visibleGroups.map((group) => <div className="settings-navigation__group" key={group.label}>
        <p>{group.label}</p>
        {group.items.map((item) => <div key={item.id}>
          <button type="button" aria-current={item.id === activeId ? "page" : undefined}
            className="settings-navigation__item" onClick={() => selectSection(item.id)}>
            <BootstrapIcon className="settings-navigation__icon" name={item.icon} size={17} />
            <span className="settings-navigation__text"><strong>{item.label}</strong><small>{item.description}</small></span>
          </button>
          {matches.filter((match) => match.section === item.id).map((match) =>
            <button type="button" className="settings-navigation__match" key={match.featureId}
              aria-label={`定位设置：${match.label}`} onClick={() => selectFeature(match)}>{match.label}</button>,
          )}
        </div>)}
      </div>)}
      {visibleGroups.length === 0 && <p className="settings-navigation__empty" role="status">没有找到匹配的设置。</p>}
    </nav>
  </aside>
}
