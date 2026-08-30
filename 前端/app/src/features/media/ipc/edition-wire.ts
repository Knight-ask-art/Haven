import type {
  EditionDetailDto,
  EditionGetRequest,
  EditionListByWorkRequest,
  EditionSummaryDto,
  PageDto,
} from "../../../lib/ipc/generated/wire"

/** Thin aliases only; DTO shapes are authoritative in generated/wire.ts. */
export type { EditionDetailDto, EditionGetRequest, EditionListByWorkRequest, EditionSummaryDto }
export type EditionListByWorkResultDto = PageDto<EditionSummaryDto>
