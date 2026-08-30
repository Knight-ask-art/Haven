import { describe, expect, it } from "vitest"
import { HavenError } from "@/lib/ipc/errors"
import {
  htmlToPlainText,
  MAX_EPUB_BYTES,
  parseEpubBook,
} from "./epub-content"

interface ZipFixtureEntry {
  name: string
  bytes: Uint8Array
  method?: 0 | 8
  flags?: number
  reportedUncompressedSize?: number
}

function writeU16(target: number[], value: number): void {
  target.push(value & 0xff, (value >>> 8) & 0xff)
}

function writeU32(target: number[], value: number): void {
  target.push(value & 0xff, (value >>> 8) & 0xff, (value >>> 16) & 0xff, (value >>> 24) & 0xff)
}

function appendBytes(target: number[], bytes: Uint8Array): void {
  for (const byte of bytes) target.push(byte)
}

/** Raw DEFLATE stored blocks keep this fixture self-contained and deterministic. */
function deflateStoredBlocks(bytes: Uint8Array): Uint8Array {
  const output: number[] = []
  let offset = 0
  do {
    const size = Math.min(0xffff, bytes.byteLength - offset)
    const final = offset + size >= bytes.byteLength
    output.push(final ? 0x01 : 0x00)
    writeU16(output, size)
    writeU16(output, (~size) & 0xffff)
    appendBytes(output, bytes.subarray(offset, offset + size))
    offset += size
  } while (offset < bytes.byteLength)
  if (bytes.byteLength === 0) output.push(0x01, 0x00, 0x00, 0xff, 0xff)
  return Uint8Array.from(output)
}

function crc32(bytes: Uint8Array): number {
  let value = 0xffffffff
  for (const byte of bytes) {
    value ^= byte
    for (let bit = 0; bit < 8; bit += 1) value = (value >>> 1) ^ (value & 1 ? 0xedb88320 : 0)
  }
  return (value ^ 0xffffffff) >>> 0
}

function buildStoredZip(entries: ZipFixtureEntry[]): ArrayBuffer {
  const encoder = new TextEncoder()
  const output: number[] = []
  const central: number[] = []
  const offsets: number[] = []
  for (const entry of entries) {
    const name = encoder.encode(entry.name)
    const method = entry.method ?? 0
    const compressed = method === 8 ? deflateStoredBlocks(entry.bytes) : entry.bytes
    const offset = output.length
    offsets.push(offset)
    output.push(0x50, 0x4b, 0x03, 0x04)
    writeU16(output, 20)
    writeU16(output, entry.flags ?? 0)
    writeU16(output, method)
    writeU16(output, 0)
    writeU16(output, 0)
    writeU32(output, crc32(entry.bytes))
    writeU32(output, compressed.byteLength)
    writeU32(output, entry.bytes.byteLength)
    writeU16(output, name.byteLength)
    writeU16(output, 0)
    output.push(...name)
    appendBytes(output, compressed)
  }
  const centralOffset = output.length
  entries.forEach((entry, index) => {
    const name = encoder.encode(entry.name)
    const method = entry.method ?? 0
    const compressed = method === 8 ? deflateStoredBlocks(entry.bytes) : entry.bytes
    central.push(0x50, 0x4b, 0x01, 0x02)
    writeU16(central, 20)
    writeU16(central, 20)
    writeU16(central, entry.flags ?? 0)
    writeU16(central, method)
    writeU16(central, 0)
    writeU16(central, 0)
    writeU32(central, crc32(entry.bytes))
    writeU32(central, compressed.byteLength)
    writeU32(central, entry.reportedUncompressedSize ?? entry.bytes.byteLength)
    writeU16(central, name.byteLength)
    writeU16(central, 0)
    writeU16(central, 0)
    writeU16(central, 0)
    writeU16(central, 0)
    writeU32(central, 0)
    writeU32(central, offsets[index])
    central.push(...name)
  })
  output.push(...central)
  output.push(0x50, 0x4b, 0x05, 0x06)
  writeU16(output, 0)
  writeU16(output, 0)
  writeU16(output, entries.length)
  writeU16(output, entries.length)
  writeU32(output, central.length)
  writeU32(output, centralOffset)
  writeU16(output, 0)
  return Uint8Array.from(output).buffer
}

function text(value: string): Uint8Array {
  return new TextEncoder().encode(value)
}

interface EpubFixtureOptions {
  firstMethod?: 0 | 8
  firstReportedUncompressedSize?: number
  mimetypeFirst?: boolean
}

function validEpub(overrides: Partial<Record<string, string>> = {}, options: EpubFixtureOptions = {}): ArrayBuffer {
  const entries: ZipFixtureEntry[] = [
    { name: "mimetype", bytes: text("application/epub+zip") },
    { name: "META-INF/", bytes: new Uint8Array() },
    { name: "META-INF/container.xml", bytes: text("<?xml version=\"1.0\"?><container><rootfiles><rootfile full-path=\"OPS/package.opf\" media-type=\"application/oebps-package+xml\"/></rootfiles></container>") },
    { name: "OPS/", bytes: new Uint8Array() },
    { name: "OPS/package.opf", bytes: text(overrides.opf ?? "<package><metadata><dc:title>真实 EPUB</dc:title></metadata><manifest><item id=\"c1\" href=\"chapter-one.xhtml\" media-type=\"application/xhtml+xml\"/><item id=\"c2\" href=\"chapter-two.xhtml\" media-type=\"text/html\"/></manifest><spine><itemref idref=\"c1\"/><itemref idref=\"c2\"/></spine></package>") },
    { name: "OPS/chapter-one.xhtml", bytes: text(overrides.first ?? "<html><head><title>第一章</title></head><body><h1>第一章</h1><p>第一段 &amp; 内容。</p><p>第二段。</p><script>alert('blocked')</script><iframe>blocked</iframe></body></html>"), method: options.firstMethod, reportedUncompressedSize: options.firstReportedUncompressedSize },
    { name: "OPS/chapter-two.xhtml", bytes: text(overrides.second ?? "<html><body><h2>第二章</h2><p>后续正文。</p></body></html>") },
  ]
  if (options.mimetypeFirst === false) {
    const mimetype = entries.shift()
    if (mimetype) entries.push(mimetype)
  }
  return buildStoredZip(entries)
}

describe("epub-content", () => {
  it("parses package/spine order and renders inert chapter text", async () => {
    const publication = await parseEpubBook(validEpub())

    expect(publication.title).toBe("真实 EPUB")
    expect(publication.chapters).toHaveLength(2)
    expect(publication.chapters[0]).toMatchObject({
      id: "epub-chapter-1",
      title: "第一章",
      paragraphs: ["第一段 & 内容。", "第二段。"],
    })
    expect(publication.chapters[0].paragraphs.join(" ")).not.toContain("alert")
    expect(publication.chapters[1].title).toBe("第二章")
  })

  it("rejects an invalid spine reference instead of falling back to another chapter", async () => {
    const bytes = validEpub({
      opf: "<package><manifest><item id=\"c1\" href=\"chapter-one.xhtml\" media-type=\"application/xhtml+xml\"/></manifest><spine><itemref idref=\"c1\"/><itemref idref=\"missing\"/></spine></package>",
    })
    await expect(parseEpubBook(bytes)).rejects.toMatchObject({ code: "FORMAT_UNSUPPORTED" })
  })

  it("rejects traversal and active XML constructs", async () => {
    const traversal = validEpub({
      opf: "<package><manifest><item id=\"c1\" href=\"../outside.xhtml\" media-type=\"application/xhtml+xml\"/></manifest><spine><itemref idref=\"c1\"/></spine></package>",
    })
    await expect(parseEpubBook(traversal)).rejects.toMatchObject({ code: "SECURITY_POLICY_DENIED" })

    const entity = validEpub({
      opf: "<!DOCTYPE package [<!ENTITY xxe SYSTEM \"file:///secret\">]><package><manifest><item id=\"c1\" href=\"chapter-one.xhtml\" media-type=\"application/xhtml+xml\"/></manifest><spine><itemref idref=\"c1\"/></spine></package>",
    })
    await expect(parseEpubBook(entity)).rejects.toMatchObject({ code: "SECURITY_POLICY_DENIED" })
  })

  function minimalEpub(href: string, entries: ZipFixtureEntry[]): ArrayBuffer {
    return buildStoredZip([
      { name: "mimetype", bytes: text("application/epub+zip") },
      { name: "META-INF/container.xml", bytes: text("<?xml version=\"1.0\"?><container><rootfiles><rootfile full-path=\"package.opf\" media-type=\"application/oebps-package+xml\"/></rootfiles></container>") },
      { name: "package.opf", bytes: text(`<package><manifest><item id="c1" href="${href}" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/></spine></package>`) },
      ...entries,
    ])
  }

  it("decodes percent-encoded manifest hrefs before archive lookup", async () => {
    const publication = await parseEpubBook(minimalEpub("chapter%20one.xhtml", [
      { name: "chapter one.xhtml", bytes: text("<html><body><h1>章节</h1><p>正文。</p></body></html>") },
    ]))
    expect(publication.chapters).toHaveLength(1)
    expect(publication.chapters[0].title).toBe("章节")
  })

  it("rejects encoded traversal after percent-decoding", async () => {
    await expect(parseEpubBook(minimalEpub("%2e%2e%2foutside.xhtml", [
      { name: "chapter.xhtml", bytes: text("<html><body><p>x</p></body></html>") },
    ]))).rejects.toMatchObject({ code: "SECURITY_POLICY_DENIED" })

    await expect(parseEpubBook(minimalEpub("%2fetc%2ftarget.xhtml", [
      { name: "chapter.xhtml", bytes: text("<html><body><p>x</p></body></html>") },
    ]))).rejects.toMatchObject({ code: "SECURITY_POLICY_DENIED" })
  })

  it("rejects malformed percent escapes in manifest hrefs", async () => {
    await expect(parseEpubBook(minimalEpub("chapter%.xhtml", [
      { name: "chapter.xhtml", bytes: text("<html><body><p>x</p></body></html>") },
    ]))).rejects.toMatchObject({ code: "FORMAT_UNSUPPORTED" })
  })

  it("rejects duplicated central-directory entry names", async () => {
    const body = "<html><body><p>A</p></body></html>"
    await expect(parseEpubBook(minimalEpub("chapter.xhtml", [
      { name: "chapter.xhtml", bytes: text(body) },
      { name: "chapter.xhtml", bytes: text(body) },
    ]))).rejects.toMatchObject({ code: "SECURITY_POLICY_DENIED" })
  })

  it("rejects encrypted entries flagged in the central directory", async () => {
    await expect(parseEpubBook(buildStoredZip([
      { name: "mimetype", bytes: text("application/epub+zip"), flags: 1 },
      { name: "META-INF/container.xml", bytes: text("<?xml version=\"1.0\"?><container><rootfiles/></container>") },
    ]))).rejects.toMatchObject({ code: "SECURITY_POLICY_DENIED" })
  })

  // This case deliberately inflates a 4 MiB payload to prove the decompressed
  // bound is enforced rather than trusting ZIP metadata. Give the bounded
  // compression work a dedicated timeout so full-suite scheduling cannot make
  // the safety regression test flaky while keeping the same assertion.
  it("requires mimetype as the first central-directory entry and bounds actual deflate output", async () => {
    await expect(parseEpubBook(validEpub({}, { mimetypeFirst: false }))).rejects.toMatchObject({ code: "FORMAT_UNSUPPORTED" })

    const deflated = await parseEpubBook(validEpub({}, { firstMethod: 8 }))
    expect(deflated.chapters[0].paragraphs).toContain("第一段 & 内容。")

    const oversized = "x".repeat(4 * 1024 * 1024 + 1)
    await expect(parseEpubBook(validEpub({ first: `<html><body><p>${oversized}</p></body></html>` }, {
      firstMethod: 8,
      // Keep the central size below the limit so the reader must enforce the
      // bound against actual decompressed chunks, not only metadata.
      firstReportedUncompressedSize: 1,
    }))).rejects.toMatchObject({ code: "SECURITY_POLICY_DENIED" })
  }, 90_000)

  it("handles empty chapters and aborts before reading", async () => {
    const empty = await parseEpubBook(validEpub({ first: "<html><body><script>hidden</script></body></html>", second: "<html><body></body></html>" }))
    expect(empty.chapters).toEqual([])

    const controller = new AbortController()
    controller.abort()
    await expect(parseEpubBook(validEpub(), controller.signal)).rejects.toMatchObject({ code: "OPERATION_CANCELLED" })
  })

  it("cuts Gutenberg license boilerplate appended after the END marker", async () => {
    const publication = await parseEpubBook(validEpub({
      second: "<html><body><h2>第二章</h2>"
        + "<p>后续正文。</p>"
        + "<p>*** END OF THE PROJECT GUTENBERG EBOOK FRANKENSTEIN ***</p>"
        + "<p>Most people start at our website which has the main PG search facility: www.gutenberg.org.</p>"
        + "<p>Section 5. General Information About Project Gutenberg.</p>"
        + "</body></html>",
    }))
    expect(publication.chapters[1].paragraphs).toEqual(["后续正文。"])
    expect(publication.chapters[1].paragraphs.join(" ")).not.toContain("gutenberg.org")
    expect(publication.chapters[1].paragraphs.join(" ")).not.toContain("General Information")
  })

  it("skips whole Gutenberg front-matter documents", async () => {
    const publication = await parseEpubBook(validEpub({
      first: "<html><body><h1>The Project Gutenberg eBook of Frankenstein</h1>"
        + "<p>*** START OF THE PROJECT GUTENBERG EBOOK FRANKENSTEIN ***</p>"
        + "<p>donations to the Project Gutenberg Literary Archive Foundation.</p>"
        + "</body></html>",
    }))
    expect(publication.chapters).toHaveLength(1)
    expect(publication.chapters[0].title).toBe("第二章")
  })

  it("enforces the bounded resource input", async () => {
    const tooLarge = new ArrayBuffer(MAX_EPUB_BYTES + 1)
    await expect(parseEpubBook(tooLarge)).rejects.toMatchObject({ code: "FORMAT_UNSUPPORTED" })
  })

  it("removes active tags and decodes safe text entities", () => {
    expect(htmlToPlainText("<p>A&nbsp;&amp; B</p><script>bad</script><p>C</p>")).toBe("A & B\n\nC")
    expect(htmlToPlainText("<iframe src=\"https://evil\">bad</iframe><p>safe</p>")).toBe("safe")
  })

  it("rejects numeric entities outside Unicode scalar values", async () => {
    for (const entity of ["&#x110000;", "&#xD800;"]) {
      const error = (() => {
        try {
          htmlToPlainText(`<p>${entity}</p>`)
          return null
        } catch (value: unknown) {
          return value
        }
      })()
      expect(error).toBeInstanceOf(HavenError)
      expect((error as HavenError).code).toBe("FORMAT_UNSUPPORTED")
    }

    const invalidXmlEntity = validEpub({
      opf: "<package><manifest><item id=\"c1\" href=\"chapter-one.xhtml&#x110000;\" media-type=\"application/xhtml+xml\"/></manifest><spine><itemref idref=\"c1\"/></spine></package>",
    })
    await expect(parseEpubBook(invalidXmlEntity)).rejects.toMatchObject({ code: "FORMAT_UNSUPPORTED" })
  })

  it("uses stable HavenError codes for malformed content", async () => {
    const error = await parseEpubBook(new ArrayBuffer(0)).catch((value: unknown) => value)
    expect(error).toBeInstanceOf(HavenError)
    expect((error as HavenError).code).toBe("FORMAT_UNSUPPORTED")
  })
})
