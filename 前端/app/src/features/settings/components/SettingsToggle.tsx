import { cn } from "@/lib/utils"

/** 既有设置开关；紧凑尺寸用于下载策略表单。 */
export function SettingsToggle({ checked, onChange, label, disabled = false, compact = false }: {
  checked: boolean; onChange: (value: boolean) => void; label: string; disabled?: boolean; compact?: boolean
}) {
  return <button type="button" role="switch" aria-checked={checked} aria-label={label} disabled={disabled}
    onClick={() => onChange(!checked)} className={cn(
      "relative rounded-full border border-[var(--haven-settings-border-subtle)] p-[2px] transition-colors duration-300 ease-in-out focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--haven-settings-primary)]",
      compact ? "h-[24px] w-[42px]" : "h-[32px] w-[52px]",
      checked ? "bg-[var(--haven-settings-primary)]" : "bg-[var(--haven-settings-toggle-off)] enabled:hover:bg-[var(--haven-settings-toggle-off-hover)]",
      disabled && "cursor-not-allowed opacity-50",
    )}>
    <span className={cn(
      "block rounded-full shadow-[0_2px_6px_rgba(0,0,0,0.15),0_0_1px_rgba(0,0,0,0.2)] transition-transform duration-300 ease-in-out",
      checked ? "bg-[var(--haven-settings-toggle-thumb-on)]" : "bg-[var(--haven-settings-toggle-thumb)]",
      compact ? "h-[18px] w-[18px]" : "h-[26px] w-[26px]",
      checked && (compact ? "translate-x-[18px]" : "translate-x-[20px]"),
    )} />
  </button>
}
