import type { ReactNode } from "react"
import type { FeatureId } from "../lib/settings-registry"
import "./storage-downloads.css"

/** 下载、存储的大表单布局；复用 Settings shell 的配色与 Dock 安全区。 */
export function ManagementHeading({ title, description }: { title: string; description: string }) {
  return <header className="storage-management-heading"><h2>{title}</h2><p>{description}</p></header>
}

export function ManagementSection({ title, description, action, children, feature }: {
  title: string; description: string; action?: ReactNode; children: ReactNode; feature?: FeatureId
}) {
  return <section className="storage-management-section" aria-label={title} data-settings-features={feature} tabIndex={feature ? -1 : undefined}>
    <header><div><h3>{title}</h3><p>{description}</p></div>{action}</header>
    {children}
  </section>
}
