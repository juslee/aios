//! Generic collection types: FixedQueue, RingBuffer.
//!
//! Used by scheduler run queues and IPC message rings.

// ---------------------------------------------------------------------------
// FixedQueue — Copy-based circular buffer (used by scheduler run queues)
// ---------------------------------------------------------------------------

/// Simple FIFO circular buffer with compile-time capacity.
///
/// Used by the scheduler for per-class run queues. Generic over element type
/// and capacity so it can be tested on the host and reused across subsystems.
pub struct FixedQueue<T: Copy, const N: usize> {
    buf: [Option<T>; N],
    head: usize,
    tail: usize,
    len: usize,
}

impl<T: Copy, const N: usize> Default for FixedQueue<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Copy, const N: usize> FixedQueue<T, N> {
    /// Create an empty queue.
    pub const fn new() -> Self {
        Self {
            buf: [None; N],
            head: 0,
            tail: 0,
            len: 0,
        }
    }

    /// Push an element to the back. Returns false if full.
    pub fn push_back(&mut self, val: T) -> bool {
        if self.len >= N {
            return false;
        }
        self.buf[self.tail] = Some(val);
        self.tail = (self.tail + 1) % N;
        self.len += 1;
        true
    }

    /// Pop an element from the front. Returns None if empty.
    pub fn pop_front(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        let val = self.buf[self.head].take();
        self.head = (self.head + 1) % N;
        self.len -= 1;
        val
    }

    /// Returns true if the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns the number of elements in the queue.
    pub fn len(&self) -> usize {
        self.len
    }
}

// ---------------------------------------------------------------------------
// RingBuffer — Clone-based circular buffer (used by IPC message queues)
// ---------------------------------------------------------------------------

/// Fixed-capacity ring buffer.
///
/// Unlike `FixedQueue` (which uses `Option<T>` + `Copy`), this stores
/// elements directly. Requires `Copy` for construction (array fill) and
/// `Clone` on the impl bound for consistency. Push/pop move values by
/// assignment (no explicit `.clone()`). Used by IPC `MessageRing` where
/// elements are large (256+ bytes).
pub struct RingBuffer<T, const N: usize> {
    buf: [T; N],
    head: usize,
    tail: usize,
    len: usize,
}

impl<T: Clone, const N: usize> RingBuffer<T, N> {
    /// Create a new ring buffer filled with copies of `default`.
    pub fn new(default: T) -> Self
    where
        T: Copy,
    {
        Self {
            buf: [default; N],
            head: 0,
            tail: 0,
            len: 0,
        }
    }

    /// Push an element to the back. Returns false if full.
    pub fn push(&mut self, val: T) -> bool {
        if self.len >= N {
            return false;
        }
        self.buf[self.tail] = val;
        self.tail = (self.tail + 1) % N;
        self.len += 1;
        true
    }

    /// Pop an element from the front. Returns None if empty.
    pub fn pop(&mut self) -> Option<T>
    where
        T: Default,
    {
        if self.len == 0 {
            return None;
        }
        let val = core::mem::take(&mut self.buf[self.head]);
        self.head = (self.head + 1) % N;
        self.len -= 1;
        Some(val)
    }

    /// Returns true if the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns the number of elements in the queue.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns true if the queue is at capacity.
    pub fn is_full(&self) -> bool {
        self.len >= N
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── FixedQueue tests ────────────────────────────────────────────────

    #[test]
    fn queue_empty_on_new() {
        let q = FixedQueue::<u32, 8>::new();
        assert!(q.is_empty());
        assert_eq!(q.len(), 0);
    }

    #[test]
    fn queue_push_pop_basic() {
        let mut q = FixedQueue::<u32, 8>::new();
        assert!(q.push_back(10));
        assert!(q.push_back(20));
        assert!(q.push_back(30));
        assert_eq!(q.len(), 3);
        assert!(!q.is_empty());

        assert_eq!(q.pop_front(), Some(10));
        assert_eq!(q.pop_front(), Some(20));
        assert_eq!(q.pop_front(), Some(30));
        assert_eq!(q.pop_front(), None);
        assert!(q.is_empty());
    }

    #[test]
    fn queue_fifo_order() {
        let mut q = FixedQueue::<u32, 64>::new();
        for i in 0..50 {
            assert!(q.push_back(i));
        }
        for i in 0..50 {
            assert_eq!(q.pop_front(), Some(i));
        }
    }

    #[test]
    fn queue_full_rejects() {
        let mut q = FixedQueue::<u32, 4>::new();
        assert!(q.push_back(1));
        assert!(q.push_back(2));
        assert!(q.push_back(3));
        assert!(q.push_back(4));
        assert!(!q.push_back(5));
        assert_eq!(q.len(), 4);
    }

    #[test]
    fn queue_wraparound() {
        let mut q = FixedQueue::<u32, 4>::new();
        for round in 0..3 {
            let base = round * 10;
            assert!(q.push_back(base + 1));
            assert!(q.push_back(base + 2));
            assert!(q.push_back(base + 3));
            assert_eq!(q.pop_front(), Some(base + 1));
            assert_eq!(q.pop_front(), Some(base + 2));
            assert_eq!(q.pop_front(), Some(base + 3));
            assert!(q.is_empty());
        }
    }

    #[test]
    fn queue_interleaved_push_pop() {
        let mut q = FixedQueue::<u32, 4>::new();
        assert!(q.push_back(1));
        assert!(q.push_back(2));
        assert_eq!(q.pop_front(), Some(1));
        assert!(q.push_back(3));
        assert!(q.push_back(4));
        assert_eq!(q.pop_front(), Some(2));
        assert!(q.push_back(5));
        assert_eq!(q.pop_front(), Some(3));
        assert_eq!(q.pop_front(), Some(4));
        assert_eq!(q.pop_front(), Some(5));
        assert!(q.is_empty());
    }

    #[test]
    fn queue_capacity_1() {
        let mut q = FixedQueue::<u32, 1>::new();
        assert!(q.push_back(42));
        assert!(!q.push_back(99));
        assert_eq!(q.pop_front(), Some(42));
        assert!(q.push_back(99));
        assert_eq!(q.pop_front(), Some(99));
    }

    #[test]
    fn queue_pop_empty() {
        let mut q = FixedQueue::<u32, 8>::new();
        assert_eq!(q.pop_front(), None);
        assert_eq!(q.pop_front(), None);
        q.push_back(1);
        q.pop_front();
        assert_eq!(q.pop_front(), None);
    }

    #[test]
    fn queue_fill_drain_cycles() {
        let mut q = FixedQueue::<u32, 8>::new();
        for cycle in 0..10u32 {
            for i in 0..8 {
                assert!(q.push_back(cycle * 100 + i));
            }
            assert!(!q.push_back(999));
            assert_eq!(q.len(), 8);
            for i in 0..8 {
                assert_eq!(q.pop_front(), Some(cycle * 100 + i));
            }
            assert!(q.is_empty());
        }
    }

    // ── RingBuffer tests ────────────────────────────────────────────────

    #[test]
    fn ring_empty_on_new() {
        let r = RingBuffer::<u32, 8>::new(0);
        assert!(r.is_empty());
        assert_eq!(r.len(), 0);
        assert!(!r.is_full());
    }

    #[test]
    fn ring_push_pop_basic() {
        let mut r = RingBuffer::<u32, 8>::new(0);
        assert!(r.push(10));
        assert!(r.push(20));
        assert!(r.push(30));
        assert_eq!(r.len(), 3);
        assert!(!r.is_empty());

        assert_eq!(r.pop(), Some(10));
        assert_eq!(r.pop(), Some(20));
        assert_eq!(r.pop(), Some(30));
        assert_eq!(r.pop(), None);
        assert!(r.is_empty());
    }

    #[test]
    fn ring_fifo_order() {
        let mut r = RingBuffer::<u32, 64>::new(0);
        for i in 0..50 {
            assert!(r.push(i));
        }
        for i in 0..50 {
            assert_eq!(r.pop(), Some(i));
        }
    }

    #[test]
    fn ring_full_rejects() {
        let mut r = RingBuffer::<u32, 4>::new(0);
        assert!(r.push(1));
        assert!(r.push(2));
        assert!(r.push(3));
        assert!(r.push(4));
        assert!(!r.push(5));
        assert_eq!(r.len(), 4);
        assert!(r.is_full());
    }

    #[test]
    fn ring_wraparound() {
        let mut r = RingBuffer::<u32, 4>::new(0);
        for round in 0..5u32 {
            let base = round * 10;
            assert!(r.push(base + 1));
            assert!(r.push(base + 2));
            assert!(r.push(base + 3));
            assert_eq!(r.pop(), Some(base + 1));
            assert_eq!(r.pop(), Some(base + 2));
            assert_eq!(r.pop(), Some(base + 3));
            assert!(r.is_empty());
        }
    }

    #[test]
    fn ring_interleaved() {
        let mut r = RingBuffer::<u32, 4>::new(0);
        assert!(r.push(1));
        assert!(r.push(2));
        assert_eq!(r.pop(), Some(1));
        assert!(r.push(3));
        assert!(r.push(4));
        assert_eq!(r.pop(), Some(2));
        assert!(r.push(5));
        assert_eq!(r.pop(), Some(3));
        assert_eq!(r.pop(), Some(4));
        assert_eq!(r.pop(), Some(5));
        assert!(r.is_empty());
    }

    #[test]
    fn ring_capacity_1() {
        let mut r = RingBuffer::<u32, 1>::new(0);
        assert!(r.push(42));
        assert!(!r.push(99));
        assert!(r.is_full());
        assert_eq!(r.pop(), Some(42));
        assert!(r.push(99));
        assert_eq!(r.pop(), Some(99));
    }

    #[test]
    fn ring_fill_drain_cycles() {
        let mut r = RingBuffer::<u32, 8>::new(0);
        for cycle in 0..10u32 {
            for i in 0..8 {
                assert!(r.push(cycle * 100 + i));
            }
            assert!(!r.push(999));
            assert_eq!(r.len(), 8);
            for i in 0..8 {
                assert_eq!(r.pop(), Some(cycle * 100 + i));
            }
            assert!(r.is_empty());
        }
    }

    #[test]
    fn ring_with_clone_type() {
        #[derive(Clone, Copy, Default, PartialEq, Debug)]
        struct Msg {
            data: [u8; 32],
            len: usize,
        }

        let mut r = RingBuffer::<Msg, 4>::new(Msg {
            data: [0; 32],
            len: 0,
        });

        let mut m1 = Msg {
            data: [0; 32],
            len: 5,
        };
        m1.data[0..5].copy_from_slice(b"hello");

        assert!(r.push(m1));
        let out = r.pop().unwrap();
        assert_eq!(out.len, 5);
        assert_eq!(&out.data[0..5], b"hello");
    }
}
