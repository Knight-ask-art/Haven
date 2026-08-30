-- 018_progress_keyframe: 关键帧（退出时截取的当前帧）持久化
-- 仅新增列，前向兼容，老库 keyframe_uri 为 NULL 时足迹回退海报
ALTER TABLE progress ADD COLUMN keyframe_uri TEXT;
