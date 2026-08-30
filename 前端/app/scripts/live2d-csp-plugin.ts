import fs from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"
import vm from "node:vm"
import type { Plugin } from "vite"

const OML2D_MODULE_PATH = fileURLToPath(import.meta.resolve("oh-my-live2d"))
const PIXI_UNSAFE_EVAL_MODULE_PATH = fileURLToPath(import.meta.resolve("@pixi/unsafe-eval"))
const CUBISM_RUNTIME_PATH = "/live2d/vendor/cubism2-runtime.js"
const OML2D_STYLESHEET_PATH = "/live2d/vendor/oml2d.css"
const OML2D_PRIMARY_COLOR = "#007aff"
const INLINE_RUNTIME_START = 'const q0 = document.createElement("script");'
const INLINE_RUNTIME_EXPRESSION = "const H2 = "
const INLINE_RUNTIME_END = ";\nq0.append(H2);"
const REMOTE_VERSION_CHECK = "tp(), this.options.sayHello && q2()"
const REMOTE_VERSION_FETCH = 'fetch("https://unpkg.com/oh-my-live2d@latest/package.json")'
const REMOTE_TIP_FETCH = 'fetch("https://v1.hitokoto.cn")'
const GLOBAL_STYLE_START = "const rf = (i) => `"
const GLOBAL_STYLE_END = "`, U2 = {}"
const GLOBAL_STYLE_INITIALIZATION = "this.globalStyle.initialize(), "
const PIXI_UNSAFE_EVAL_VERSION = "@pixi/unsafe-eval - v6.5.10"
const PIXI_UNSAFE_EVAL_EXPORT = "export { install };"
const PIXI_UNSAFE_EVAL_SOURCE_MAP = "\n//# sourceMappingURL=unsafe-eval.mjs.map"
const EMBEDDED_PIXI_SHADER_SYSTEM_END = `    }, i.prototype.destroy = function() {
      this.renderer = null, this.destroyed = !0;
    }, i;
  }()
);`

function replaceExactlyOnce(
  source: string,
  search: string,
  replacement: string,
  description: string,
): string {
  if (source.split(search).length - 1 !== 1) {
    throw new Error(`oh-my-live2d ${description} changed; review the offline patch before building`)
  }
  return source.replace(search, replacement)
}

function normalizeModuleId(id: string): string {
  let cleanId = id.split("?", 1)[0]
  if (cleanId.startsWith("/@fs/")) cleanId = cleanId.slice(4)
  if (/^\/[A-Za-z]:\//.test(cleanId)) cleanId = cleanId.slice(1)
  return path.normalize(path.resolve(cleanId))
}

export function injectPixiUnsafeEvalPatch(
  source: string,
  unsafeEvalSource: string,
): string {
  if (unsafeEvalSource.split(PIXI_UNSAFE_EVAL_VERSION).length - 1 !== 1) {
    throw new Error("@pixi/unsafe-eval version changed; review the CSP patch before building")
  }

  let implementation = replaceExactlyOnce(
    unsafeEvalSource,
    PIXI_UNSAFE_EVAL_SOURCE_MAP,
    "",
    "source map footer",
  )
  implementation = replaceExactlyOnce(
    implementation,
    PIXI_UNSAFE_EVAL_EXPORT,
    "install({ ShaderSystem: qu });",
    "install export",
  )

  return replaceExactlyOnce(
    source,
    EMBEDDED_PIXI_SHADER_SYSTEM_END,
    `${EMBEDDED_PIXI_SHADER_SYSTEM_END}\n${implementation}`,
    "embedded Pixi ShaderSystem layout",
  )
}

type Oml2dSources = {
  cubismRuntime: string
  globalCss: string
  patchedModule: string
}

function extractGlobalCss(source: string): string {
  if (
    source.split(GLOBAL_STYLE_START).length - 1 !== 1
    || source.split(GLOBAL_STYLE_END).length - 1 !== 1
  ) {
    throw new Error("oh-my-live2d global CSS layout changed; review the CSP patch before building")
  }

  const styleStart = source.indexOf(GLOBAL_STYLE_START) + GLOBAL_STYLE_START.length
  const styleEnd = source.indexOf(GLOBAL_STYLE_END, styleStart)
  const template = source.slice(styleStart, styleEnd)
  const interpolations = template.match(/\$\{[^}]+\}/g) ?? []
  const expectedInterpolations = ["${i}", "${i}", "${_i.menus}"]

  if (
    interpolations.length !== expectedInterpolations.length
    || interpolations.some((value, index) => value !== expectedInterpolations[index])
  ) {
    throw new Error("oh-my-live2d global CSS interpolation changed; refusing to emit unreviewed CSS")
  }

  return template
    .replaceAll("${i}", OML2D_PRIMARY_COLOR)
    .replace("${_i.menus}", "oml2d-menus")
}

function prepareOml2dSources(): Oml2dSources {
  const source = fs.readFileSync(OML2D_MODULE_PATH, "utf8")
  const unsafeEvalSource = fs.readFileSync(PIXI_UNSAFE_EVAL_MODULE_PATH, "utf8")
  const globalCss = extractGlobalCss(source)
  const injectionStart = source.indexOf(INLINE_RUNTIME_START)
  const expressionStart = source.indexOf(INLINE_RUNTIME_EXPRESSION, injectionStart)
  const expressionEnd = source.indexOf(INLINE_RUNTIME_END, expressionStart)

  if (injectionStart < 0 || expressionStart < 0 || expressionEnd < 0) {
    throw new Error("oh-my-live2d runtime layout changed; review the CSP patch before building")
  }

  const runtimeExpression = source.slice(
    expressionStart + INLINE_RUNTIME_EXPRESSION.length,
    expressionEnd,
  )
  if (
    !runtimeExpression.startsWith("`")
    || !runtimeExpression.endsWith("`")
    || runtimeExpression.includes("${")
  ) {
    throw new Error("oh-my-live2d runtime is not a static template literal; refusing to evaluate it")
  }

  const cubismRuntime = vm.runInNewContext(
    runtimeExpression,
    Object.create(null),
    { timeout: 1000 },
  )
  if (typeof cubismRuntime !== "string" || !cubismRuntime.includes("Live2D")) {
    throw new Error("oh-my-live2d did not yield the expected Cubism 2 runtime")
  }

  const afterInlineRuntime = expressionEnd + INLINE_RUNTIME_END.length
  const withoutInlineRuntime = source.slice(0, injectionStart) + source.slice(afterInlineRuntime)
  let patchedModule = replaceExactlyOnce(
    withoutInlineRuntime,
    REMOTE_VERSION_CHECK,
    "this.options.sayHello && q2()",
    "remote version check",
  )
  patchedModule = replaceExactlyOnce(
    patchedModule,
    REMOTE_VERSION_FETCH,
    'Promise.resolve({ json: () => Promise.resolve({ version: "0.19.3" }) })',
    "remote version fetch",
  )
  patchedModule = replaceExactlyOnce(
    patchedModule,
    REMOTE_TIP_FETCH,
    'Promise.resolve({ json: () => Promise.resolve({ hitokoto: "" }) })',
    "remote tip fetch",
  )
  patchedModule = replaceExactlyOnce(
    patchedModule,
    GLOBAL_STYLE_INITIALIZATION,
    "",
    "dynamic global style initialization",
  )
  patchedModule = injectPixiUnsafeEvalPatch(patchedModule, unsafeEvalSource)

  return {
    cubismRuntime,
    globalCss,
    patchedModule,
  }
}

export function live2dCspPlugin(): Plugin {
  const sources = prepareOml2dSources()

  return {
    name: "haven-live2d-csp",
    enforce: "pre",
    configureServer(server) {
      server.middlewares.use((request, response, next) => {
        const requestPath = request.url?.split("?", 1)[0]
        if (requestPath !== CUBISM_RUNTIME_PATH && requestPath !== OML2D_STYLESHEET_PATH) {
          next()
          return
        }

        response.statusCode = 200
        response.setHeader(
          "Content-Type",
          requestPath === OML2D_STYLESHEET_PATH
            ? "text/css; charset=utf-8"
            : "text/javascript; charset=utf-8",
        )
        response.setHeader("Cache-Control", "no-store")
        response.end(
          requestPath === OML2D_STYLESHEET_PATH
            ? sources.globalCss
            : sources.cubismRuntime,
        )
      })
    },
    transform(_code, id) {
      if (normalizeModuleId(id) !== normalizeModuleId(OML2D_MODULE_PATH)) return null
      return { code: sources.patchedModule, map: null }
    },
    generateBundle() {
      this.emitFile({
        type: "asset",
        fileName: CUBISM_RUNTIME_PATH.slice(1),
        source: sources.cubismRuntime,
      })
      this.emitFile({
        type: "asset",
        fileName: OML2D_STYLESHEET_PATH.slice(1),
        source: sources.globalCss,
      })
    },
  }
}
