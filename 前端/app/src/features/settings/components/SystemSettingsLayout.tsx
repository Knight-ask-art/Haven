import { useId, type ReactNode } from "react"
import type { FeatureId } from "../lib/settings-registry"
import "./system-settings.css"

export function SystemSettingsIntro({ section, title, description }: {
  section: string; title: string; description: string
}) {
  return <header aria-label={section} className="system-settings__intro">
    <p className="system-settings__eyebrow">系统设置</p>
    <h2>{title}</h2>
    <p>{description}</p>
  </header>
}

export function SystemSettingsSection({ title, description, children, action, feature }: {
  title: string; description?: string; children: ReactNode; action?: ReactNode; feature?: FeatureId
}) {
  const titleId = useId()
  return <section aria-labelledby={titleId} className="system-settings__section" data-settings-features={feature} tabIndex={feature ? -1 : undefined}>
    <header className="system-settings__section-header">
      <div>
        <h3 id={titleId}>{title}</h3>
        {description && <p>{description}</p>}
      </div>
      {action}
    </header>
    <div className="system-settings__surface">{children}</div>
  </section>
}

export function SystemSettingsRow({ title, description, icon, children, feature }: {
  title: string; description?: string; icon?: ReactNode; children: ReactNode; feature?: FeatureId
}) {
  return <div className="system-settings__row" data-settings-features={feature} tabIndex={feature ? -1 : undefined}>
    <div className="system-settings__row-label">
      {icon && <span aria-hidden="true" className="system-settings__row-icon">{icon}</span>}
      <div><p className="system-settings__label">{title}</p>{description && <p className="system-settings__description">{description}</p>}</div>
    </div>
    <div className="system-settings__row-control">{children}</div>
  </div>
}
