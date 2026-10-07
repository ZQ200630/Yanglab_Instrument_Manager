pub const MAX_SEQUENCE: u64 = 9_007_199_254_740_991;
pub const MAX_REQUEST_BYTES: usize = 65536;
pub const MAX_REPLY_BYTES: usize = 17 * 1024 * 1024;
pub const MAX_TRACE_POINTS: usize = 200001;
pub const MAX_CAPTURE_CHUNK: usize = 16384;
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_domains: usize,
    pub max_request_bytes: usize,
    pub max_pending_normal: usize,
    pub max_reply_slots: usize,
    pub max_responsibility_slots: usize,
    pub ordinary_slots: usize,
    pub observation_slots: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_domains: 64,
            max_request_bytes: MAX_REQUEST_BYTES,
            max_pending_normal: 31,
            max_reply_slots: 226,
            max_responsibility_slots: 225,
            ordinary_slots: 4,
            observation_slots: 4,
        }
    }
}
