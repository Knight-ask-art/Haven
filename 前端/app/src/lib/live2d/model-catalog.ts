import {
  CHARACTER_REGISTRY,
  type Live2DCharacter,
} from "@/lib/mascotState"

const BUNDLED_MODEL_ROOT = "/live2d/models"

export interface ResolvedLive2dModel {
  id: string
  entryUrl: string
  scale: number
  position: readonly [number, number]
  source: "bundled" | "user"
}

export interface ResolvedLive2dCharacter {
  character: Live2DCharacter
  model: ResolvedLive2dModel
}

/**
 * UI 只消费目录解析后的模型。未来的导入流程可把用户选择的兼容模型包
 * 复制到应用专用 Live2D 资产目录，再用同一接口提供给首页和设置页。
 */
export interface Live2dModelCatalog {
  list(): readonly ResolvedLive2dCharacter[]
  resolve(characterId: string): ResolvedLive2dCharacter | null
}

const BUNDLED_MODELS: Readonly<Record<string, ResolvedLive2dModel>> = {
  miku: {
    id: "miku",
    entryUrl: `${BUNDLED_MODEL_ROOT}/miku/assets/miku.model.json`,
    scale: 0.15,
    position: [0, -8],
    source: "bundled",
  },
  shizuku: {
    id: "shizuku",
    entryUrl: `${BUNDLED_MODEL_ROOT}/shizuku/assets/shizuku.model.json`,
    scale: 0.16,
    position: [0, -8],
    source: "bundled",
  },
  koharu: {
    id: "koharu",
    entryUrl: `${BUNDLED_MODEL_ROOT}/koharu/assets/koharu.model.json`,
    scale: 0.15,
    position: [0, -8],
    source: "bundled",
  },
  unitychan: {
    id: "unitychan",
    entryUrl: `${BUNDLED_MODEL_ROOT}/unitychan/assets/unitychan.model.json`,
    scale: 0.14,
    position: [0, -8],
    source: "bundled",
  },
  z16: {
    id: "z16",
    entryUrl: `${BUNDLED_MODEL_ROOT}/z16/assets/z16.model.json`,
    scale: 0.2,
    position: [0, -8],
    source: "bundled",
  },
}

const bundledCharacters = CHARACTER_REGISTRY.flatMap((character) => {
  const model = BUNDLED_MODELS[character.id]
  return model ? [{ character, model }] : []
})

export const bundledLive2dModelCatalog: Live2dModelCatalog = {
  list: () => bundledCharacters,
  resolve: (characterId) => (
    bundledCharacters.find(({ character }) => character.id === characterId) ?? null
  ),
}
