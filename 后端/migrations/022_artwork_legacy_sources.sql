-- 022_artwork_legacy_sources: 为 021 之前登记的已知图片来源补齐策略。
--
-- 021 之后 Artwork Cache 对 source_id=NULL 的记录只允许读取已经存在的
-- 本地缓存，避免把历史 target_url 当成未经策略验证的出站地址。这里仅
-- 回填能够从精确 Host 确定来源策略的旧记录；未知来源继续保持 NULL，
-- 由 Artwork Cache 安全地 fail closed。

UPDATE image_proxy
SET source_id = 'cms10',
    normalized_host = 'img.picbf.com'
WHERE source_id IS NULL
  AND normalized_host IS NULL
  AND (
      lower(target_url) GLOB 'http://img.picbf.com/*'
      OR lower(target_url) GLOB 'https://img.picbf.com/*'
  )
  AND instr(lower(target_url), '#') = 0
  AND lower(target_url) NOT GLOB '*[?&]token=*'
  AND lower(target_url) NOT GLOB '*[?&]sig=*'
  AND lower(target_url) NOT GLOB '*[?&]signature=*'
  AND lower(target_url) NOT GLOB '*[?&]secret=*'
  AND lower(target_url) NOT GLOB '*[?&]key=*'
  AND lower(target_url) NOT GLOB '*[?&]auth=*'
  AND lower(target_url) NOT GLOB '*[?&]expires=*'
  AND lower(target_url) NOT GLOB '*[?&]expiry=*';

UPDATE image_proxy
SET source_id = 'opds',
    normalized_host = CASE
        WHEN lower(target_url) GLOB 'https://www.gutenberg.org/*'
          OR lower(target_url) GLOB 'http://www.gutenberg.org/*'
        THEN 'www.gutenberg.org'
        ELSE 'gutenberg.org'
    END
WHERE source_id IS NULL
  AND normalized_host IS NULL
  AND (
      lower(target_url) GLOB 'http://www.gutenberg.org/*'
      OR lower(target_url) GLOB 'https://www.gutenberg.org/*'
      OR lower(target_url) GLOB 'http://gutenberg.org/*'
      OR lower(target_url) GLOB 'https://gutenberg.org/*'
  )
  AND instr(lower(target_url), '#') = 0
  AND lower(target_url) NOT GLOB '*[?&]token=*'
  AND lower(target_url) NOT GLOB '*[?&]sig=*'
  AND lower(target_url) NOT GLOB '*[?&]signature=*'
  AND lower(target_url) NOT GLOB '*[?&]secret=*'
  AND lower(target_url) NOT GLOB '*[?&]key=*'
  AND lower(target_url) NOT GLOB '*[?&]auth=*'
  AND lower(target_url) NOT GLOB '*[?&]expires=*'
  AND lower(target_url) NOT GLOB '*[?&]expiry=*';
