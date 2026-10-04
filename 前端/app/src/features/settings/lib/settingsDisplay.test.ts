import { describe, expect, it } from "vitest"
import type { ReadingSettingsValue } from "@/lib/ipc/settings-wire"
import {
  MAX_CUSTOM_FONT_FAMILY_LENGTH,
  READING_CUSTOM_BACKGROUND_FALLBACK,
  READING_CUSTOM_TEXT_FALLBACK,
  resolveReadingPresentation,
} from "../../reader/lib/reading-settings-mapping"
import {
  READING_CUSTOM_FONT_HINT,
  READING_CUSTOM_FONT_INVALID_HINT,
  READING_FONT_OPTIONS,
  READING_FONT_PICKER_OPTIONS,
  READING_FONT_PREVIEW_CLASS,
  READING_PAGE_SUBMODE_OPTIONS,
  READING_PAGINATION_MODE_OPTIONS,
  READING_PAGINATION_OPTIONS,
  READING_THEME_OPTIONS,
  customFontFamilyCommit,
  customReadingColorInputValue,
  customReadingColorPatch,
  optionLabel,
  optionValue,
  readingFontAlias,
  readingFontLabel,
  readingFontPickerOptions,
  readingPageSubMode,
  readingPaginationMode,
  readingPaginationWire,
  readingPreviewFontLabel,
  readingPreviewMeasureRatio,
  readingPreviewPalette,
  readingThemePatch,
  readingThemeSwatch,
} from "./settingsDisplay"

describe("custom reading colour patch", () => {
  it("normalises the picked colour without touching the theme", () => {
    expect(customReadingColorPatch("customBackground", "#A1B2C3")).toEqual({
      section: "reading",
      // 期望值里刻意没有 theme：取色器不得把选中的预设主题改成 custom。
      customBackground: "#a1b2c3",
    })
    expect(customReadingColorPatch("customText", "#001122")).toEqual({
      section: "reading",
      // 期望值里刻意没有 theme：取色器不得把选中的预设主题改成 custom。
      customText: "#001122",
    })
  })

  it("drops input that is not a contract colour instead of writing it to the draft", () => {
    expect(customReadingColorPatch("customBackground", "")).toBeNull()
    expect(customReadingColorPatch("customBackground", "red")).toBeNull()
    expect(customReadingColorPatch("customBackground", "#fff")).toBeNull()
    expect(customReadingColorPatch("customText", "rgb(0,0,0)")).toBeNull()
  })

  it("shows a renderable value for an unset or drifted stored colour", () => {
    expect(customReadingColorInputValue("#A1B2C3", READING_CUSTOM_BACKGROUND_FALLBACK)).toBe("#a1b2c3")
    expect(customReadingColorInputValue(null, READING_CUSTOM_BACKGROUND_FALLBACK)).toBe(READING_CUSTOM_BACKGROUND_FALLBACK)
    expect(customReadingColorInputValue("not-a-colour", READING_CUSTOM_BACKGROUND_FALLBACK)).toBe(READING_CUSTOM_BACKGROUND_FALLBACK)
  })
})

describe("custom font family commit", () => {
  it("saves a safe installed family name and switches the font option to custom", () => {
    expect(customFontFamilyCommit("  思源宋体  ")).toEqual({
      status: "ready",
      patch: { section: "reading", fontFamily: "custom", customFontFamily: "思源宋体" },
    })
  })

  it("only clears the stored name when the field is emptied", () => {
    expect(customFontFamilyCommit("")).toEqual({
      status: "ready",
      patch: { section: "reading", customFontFamily: null },
    })
    expect(customFontFamilyCommit("   ")).toEqual({
      status: "ready",
      patch: { section: "reading", customFontFamily: null },
    })
  })

  it("refuses unsafe input so a bad draft never reaches the snapshot", () => {
    expect(customFontFamilyCommit('Arial"; color: red')).toEqual({ status: "invalid" })
    expect(customFontFamilyCommit("Arial\\27 serif")).toEqual({ status: "invalid" })
    expect(customFontFamilyCommit("A".repeat(MAX_CUSTOM_FONT_FAMILY_LENGTH + 1))).toEqual({ status: "invalid" })
  })
})

describe("reading pagination two-level mapping", () => {
  it("splits the one wire field into a mode and a page sub-mode", () => {
    expect(readingPaginationMode("scroll")).toBe("scroll");
    expect(readingPaginationMode("paginated")).toBe("paginated");
    // 双页属于「分页」这一层，不是第三种顶层模式。
    expect(readingPaginationMode("double")).toBe("paginated");
    expect(readingPageSubMode("paginated")).toBe("paginated");
    expect(readingPageSubMode("double")).toBe("double");
    // 滚动模式下没有子选择；这里返回的只是默认取值，不会被写进契约。
    expect(readingPageSubMode("scroll")).toBe("paginated");
  });

  it("round-trips every wire value and never invents a new one", () => {
    const wires = READING_PAGINATION_OPTIONS.map((option) => option.value);
    expect(wires).toEqual(["scroll", "paginated", "double"]);
    for (const wire of wires) {
      expect(readingPaginationWire(readingPaginationMode(wire), readingPageSubMode(wire))).toBe(wire);
    }
    // 顶层选滚动时子选择不参与：契约只有一个字段，切回滚动就是 scroll。
    expect(readingPaginationWire("scroll", "double")).toBe("scroll");
    expect(readingPaginationWire("paginated", "paginated")).toBe("paginated");
    expect(readingPaginationWire("paginated", "double")).toBe("double");
  });

  it("keeps both levels' labels unique so each button maps back to one value", () => {
    expect(READING_PAGINATION_MODE_OPTIONS.map((option) => option.label)).toEqual(["连续滚动", "分页"]);
    expect(READING_PAGE_SUBMODE_OPTIONS.map((option) => option.label)).toEqual(["单页", "双页"]);
    for (const option of READING_PAGINATION_MODE_OPTIONS) {
      expect(optionValue(READING_PAGINATION_MODE_OPTIONS, option.label)).toBe(option.value);
    }
    for (const option of READING_PAGE_SUBMODE_OPTIONS) {
      expect(optionValue(READING_PAGE_SUBMODE_OPTIONS, option.label)).toBe(option.value);
    }
  });
});

function previewSettings(overrides: Partial<ReadingSettingsValue> = {}): ReadingSettingsValue {
  return {
    section: "reading",
    fontFamily: "serif",
    customFontFamily: null,
    fontSize: "medium",
    lineHeight: "comfortable",
    contentWidth: "medium",
    theme: "warm",
    customBackground: null,
    customText: null,
    fontWeight: "regular",
    letterSpacing: "normal",
    systemAuto: true,
    pagination: "scroll",
    ...overrides,
  };
}

/** 预览吃的是阅读器同一份投影，所以这里直接用 resolveReadingPresentation 造输入。 */
function previewOf(overrides: Partial<ReadingSettingsValue> = {}) {
  return resolveReadingPresentation(previewSettings(overrides), false);
}

describe("reading preview mapping", () => {
  it("uses the reader's own palette for preset themes", () => {
    expect(readingPreviewPalette(previewOf({ theme: "sepia" })))
      .toEqual({ background: "#f4ecd8", color: "#5b4636" });
    expect(readingPreviewPalette(previewOf({ theme: "dark" })))
      .toEqual({ background: "#0f0f11", color: "#d4d4d8" });
  });

  it("resolves the system theme to a concrete preset instead of leaving it unresolved", () => {
    expect(previewOf({ theme: "system" }).theme).toBe("warm");
    expect(readingPreviewPalette(previewOf({ theme: "system" })))
      .toEqual({ background: "#f5efe3", color: "#3c332b" });
  });

  it("falls back per field when the custom colours are unset or drifted", () => {
    expect(readingPreviewPalette(previewOf({ theme: "custom", customBackground: null, customText: null })))
      .toEqual({ background: READING_CUSTOM_BACKGROUND_FALLBACK, color: READING_CUSTOM_TEXT_FALLBACK });
    expect(readingPreviewPalette(previewOf({ theme: "custom", customBackground: "not-a-colour", customText: "#001122" })))
      .toEqual({ background: READING_CUSTOM_BACKGROUND_FALLBACK, color: "#001122" });
  });

  it("scales the preview measure with the reader's real content width", () => {
    const narrow = readingPreviewMeasureRatio(previewOf({ contentWidth: "narrow" }));
    const medium = readingPreviewMeasureRatio(previewOf({ contentWidth: "medium" }));
    const wide = readingPreviewMeasureRatio(previewOf({ contentWidth: "wide" }));
    expect(narrow).toBeLessThan(medium);
    expect(medium).toBeLessThan(wide);
    expect(wide).toBe(1);
    expect(narrow).toBeGreaterThan(0);
  });

  it("labels the sample with the resolved font, and says so when the custom name is unusable", () => {
    expect(readingPreviewFontLabel({ fontFamily: "serif", customFontFamily: null })).toBe("系统衬线");
    expect(readingPreviewFontLabel({ fontFamily: "custom", customFontFamily: null }))
      .toBe("自定义（未填族名，按系统无衬线渲染）");
    expect(readingPreviewFontLabel({ fontFamily: "custom", customFontFamily: 'Arial"; color: red' }))
      .toBe("自定义（未填族名，按系统无衬线渲染）");
    expect(readingPreviewFontLabel({ fontFamily: "custom", customFontFamily: "  思源宋体  " }))
      .toBe("思源宋体（自定义）");
  });

  it("covers every font option with a preview class", () => {
    for (const option of READING_FONT_OPTIONS) {
      expect(READING_FONT_PREVIEW_CLASS[option.value].length).toBeGreaterThan(0);
    }
    // 自定义字体只在 inline style 里叠加真实族名；类名保持与阅读器一致的兜底。
    expect(READING_FONT_PREVIEW_CLASS.custom).toBe("font-sans");
  });

  it("swatches every theme option, preferring a stored custom colour", () => {
    for (const option of READING_THEME_OPTIONS) {
      const swatch = readingThemeSwatch(option.value, { customBackground: null, customText: null });
      expect(swatch.background.length).toBeGreaterThan(0);
      expect(swatch.color.length).toBeGreaterThan(0);
    }
    expect(readingThemeSwatch("warm", { customBackground: "#000000", customText: "#ffffff" }))
      .toEqual({ background: "#f5efe3", color: "#3c332b" });
    expect(readingThemeSwatch("custom", { customBackground: "not-a-colour", customText: "#001122" }))
      .toEqual({ background: READING_CUSTOM_BACKGROUND_FALLBACK, color: "#001122" });
  });
});

describe("reading theme patch", () => {
  it("writes systemAuto together with the system theme", () => {
    // 契约把「跟随系统」拆成 theme=system + systemAuto：只写 theme 时，历史快照里的
    // systemAuto=false 会让这张卡片渲染成固定暖光。
    expect(readingThemePatch("system")).toEqual({ section: "reading", theme: "system", systemAuto: true });
  });

  it("leaves systemAuto alone for the fixed themes", () => {
    for (const option of READING_THEME_OPTIONS) {
      if (option.value === "system") continue;
      expect(readingThemePatch(option.value)).toEqual({ section: "reading", theme: option.value });
    }
  });

  it("never invents a theme value outside the existing enum", () => {
    const wires = READING_THEME_OPTIONS.map((option) => option.value);
    expect(wires).toContain("system");
    for (const option of READING_THEME_OPTIONS) {
      expect(readingThemePatch(option.value).theme).toBe(option.value);
    }
  });
});

describe("reading font aliases", () => {
  /** 与实现无关的独立推导：类名和更早的选项相同、且不叠加 inline 族名，就是等价项。 */
  function expectedAliasedValues(): string[] {
    return READING_FONT_OPTIONS.filter((option, index) => {
      if (option.value === "custom") return false;
      return READING_FONT_OPTIONS.slice(0, index).some((earlier) => (
        earlier.value !== "custom"
        && READING_FONT_PREVIEW_CLASS[earlier.value] === READING_FONT_PREVIEW_CLASS[option.value]
      ));
    }).map((option) => option.value);
  }

  it("hides exactly the values the readers render identically to an earlier option", () => {
    expect(expectedAliasedValues()).toEqual(["fangsong", "mianfei"]);
    expect(READING_FONT_PICKER_OPTIONS.map((option) => option.value))
      .toEqual(["sans", "serif", "kai", "heiti", "custom"]);
  });

  it("names the option each hidden value is really rendered as", () => {
    expect(readingFontAlias("fangsong")).toBe("serif");
    expect(readingFontAlias("mianfei")).toBe("sans");
  });

  it("keeps every value that does change the rendered text", () => {
    for (const value of ["sans", "serif", "kai", "heiti", "custom"] as const) {
      expect(readingFontAlias(value)).toBeNull();
    }
    // custom 与 sans 的类名相同，但它会叠加 inline 族名，所以不是等价项。
    expect(READING_FONT_PREVIEW_CLASS.custom).toBe(READING_FONT_PREVIEW_CLASS.sans);
    expect(READING_FONT_PICKER_OPTIONS.map((option) => option.value)).toContain("custom");
  });

  it("keeps the wire enum itself intact for compatibility", () => {
    expect(READING_FONT_OPTIONS.map((option) => option.value))
      .toEqual(["sans", "serif", "kai", "heiti", "fangsong", "mianfei", "custom"]);
  });
});

describe("reading font picker options", () => {
  it("shows a stored legacy value and maps its label back to that same value", () => {
    const options = readingFontPickerOptions("fangsong");
    expect(options.map((option) => option.value))
      .toEqual(["sans", "serif", "kai", "heiti", "custom", "fangsong"]);
    expect(optionValue(options, readingFontLabel("fangsong"))).toBe("fangsong");
  });

  it("does not add a legacy entry for a value that is already pickable", () => {
    expect(readingFontPickerOptions("serif")).toBe(READING_FONT_PICKER_OPTIONS);
    expect(readingFontPickerOptions("custom")).toBe(READING_FONT_PICKER_OPTIONS);
  });

  it("labels every option so the select can show the stored value truthfully", () => {
    for (const legacy of ["fangsong", "mianfei"] as const) {
      const labels = readingFontPickerOptions(legacy).map((option) => option.label);
      // 标签唯一：SelectControl 用文案回查取值，重复文案会选错项。
      expect(new Set(labels).size).toBe(labels.length);
      expect(labels).toContain(readingFontLabel(legacy));
    }
    expect(readingFontLabel("fangsong")).toBe("仿宋（等同系统衬线）");
    expect(readingFontLabel("mianfei")).toBe("免费字体（等同系统无衬线）");
    expect(readingFontLabel("serif")).toBe("系统衬线");
  });

  it("spells the alias out in the preview label too", () => {
    expect(readingPreviewFontLabel({ fontFamily: "fangsong", customFontFamily: null }))
      .toBe("仿宋（等同系统衬线）");
    expect(readingPreviewFontLabel({ fontFamily: "mianfei", customFontFamily: null }))
      .toBe("免费字体（等同系统无衬线）");
  });
});

describe("custom font family hint wording", () => {
  it("asks for an installed family without claiming installation is verified", () => {
    expect(READING_CUSTOM_FONT_HINT).toContain("需要填写本机已安装字体的族名");
    expect(READING_CUSTOM_FONT_HINT).toContain("不会查询本机字体列表");
    expect(READING_CUSTOM_FONT_HINT).toContain("退回系统字体栈");
    expect(READING_CUSTOM_FONT_HINT).toContain("不支持字体文件导入");
    expect(READING_CUSTOM_FONT_INVALID_HINT).toContain("不校验是否已安装");
  });

  it("accepts a safe name the sanitizer cannot prove is installed", () => {
    // 只做 CSS 族名写法校验，所以「本机装没装」判断不了，也不该在文案里假装能判断。
    expect(customFontFamilyCommit("Definitely-Not-Installed-Font-9F3A")).toEqual({
      status: "ready",
      patch: { section: "reading", fontFamily: "custom", customFontFamily: "Definitely-Not-Installed-Font-9F3A" },
    });
  });
});

describe("reading option labels", () => {
  it("still round-trips the custom theme and font options", () => {
    expect(optionValue(READING_THEME_OPTIONS, "自定义")).toBe("custom")
    expect(optionLabel(READING_THEME_OPTIONS, "custom")).toBe("自定义")
    expect(optionValue(READING_FONT_OPTIONS, "自定义")).toBe("custom")
    expect(optionLabel(READING_FONT_OPTIONS, "custom")).toBe("自定义")
  })

  it("keeps every preset readable by label", () => {
    for (const option of READING_THEME_OPTIONS) {
      expect(optionValue(READING_THEME_OPTIONS, option.label)).toBe(option.value)
    }
    for (const option of READING_FONT_OPTIONS) {
      expect(optionValue(READING_FONT_OPTIONS, option.label)).toBe(option.value)
    }
  })
})
