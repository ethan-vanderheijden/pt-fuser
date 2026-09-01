use std::{
    fmt::{Debug, Display},
    hash::Hash,
    iter::Sum,
    marker::PhantomData,
    ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Sub, SubAssign},
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Metrics {
    pub ts: u64,
    pub cycles: u64,
    pub insn_count: u64,
}

impl Metrics {
    pub fn new(ts: u64, cycles: u64, insn_count: u64) -> Self {
        Self {
            ts,
            cycles,
            insn_count,
        }
    }

    pub fn constant(c: u64) -> Self {
        Self {
            ts: c,
            cycles: c,
            insn_count: c,
        }
    }
}

impl Add for Metrics {
    type Output = Metrics;

    fn add(self, rhs: Self) -> Self::Output {
        &self + &rhs
    }
}

impl Add for &Metrics {
    type Output = Metrics;

    fn add(self, rhs: Self) -> Self::Output {
        Metrics {
            ts: self.ts + rhs.ts,
            cycles: self.cycles + rhs.cycles,
            insn_count: self.insn_count + rhs.insn_count,
        }
    }
}

impl AddAssign for Metrics {
    fn add_assign(&mut self, rhs: Self) {
        self.ts += rhs.ts;
        self.cycles += rhs.cycles;
        self.insn_count += rhs.insn_count;
    }
}

impl Sub for Metrics {
    type Output = Metrics;

    fn sub(self, rhs: Self) -> Self::Output {
        &self - &rhs
    }
}

impl Sub for &Metrics {
    type Output = Metrics;

    fn sub(self, rhs: Self) -> Self::Output {
        Metrics {
            ts: self.ts - rhs.ts,
            cycles: self.cycles - rhs.cycles,
            insn_count: self.insn_count - rhs.insn_count,
        }
    }
}

impl SubAssign for Metrics {
    fn sub_assign(&mut self, rhs: Self) {
        self.ts -= rhs.ts;
        self.cycles -= rhs.cycles;
        self.insn_count -= rhs.insn_count;
    }
}

impl Div<u64> for Metrics {
    type Output = Metrics;

    fn div(self, rhs: u64) -> Self::Output {
        &self / rhs
    }
}

impl Div<u64> for &Metrics {
    type Output = Metrics;

    fn div(self, rhs: u64) -> Self::Output {
        Metrics {
            ts: self.ts / rhs,
            cycles: self.cycles / rhs,
            insn_count: self.insn_count / rhs,
        }
    }
}

impl DivAssign<u64> for Metrics {
    fn div_assign(&mut self, rhs: u64) {
        self.ts /= rhs;
        self.cycles /= rhs;
        self.insn_count /= rhs;
    }
}

impl Div for Metrics {
    type Output = Metrics;

    fn div(self, rhs: Self) -> Self::Output {
        &self / &rhs
    }
}

impl Div for &Metrics {
    type Output = Metrics;

    fn div(self, rhs: Self) -> Self::Output {
        Metrics {
            ts: self.ts / rhs.ts,
            cycles: self.cycles / rhs.cycles,
            insn_count: self.insn_count / rhs.insn_count,
        }
    }
}

impl DivAssign for Metrics {
    fn div_assign(&mut self, rhs: Self) {
        self.ts /= rhs.ts;
        self.cycles /= rhs.cycles;
        self.insn_count /= rhs.insn_count;
    }
}

impl Mul<u64> for Metrics {
    type Output = Metrics;

    fn mul(self, rhs: u64) -> Self::Output {
        &self * rhs
    }
}

impl Mul<u64> for &Metrics {
    type Output = Metrics;

    fn mul(self, rhs: u64) -> Self::Output {
        Metrics {
            ts: self.ts * rhs,
            cycles: self.cycles * rhs,
            insn_count: self.insn_count * rhs,
        }
    }
}

impl MulAssign<u64> for Metrics {
    fn mul_assign(&mut self, rhs: u64) {
        self.ts *= rhs;
        self.cycles *= rhs;
        self.insn_count *= rhs;
    }
}

impl Mul for Metrics {
    type Output = Metrics;

    fn mul(self, rhs: Self) -> Self::Output {
        &self * &rhs
    }
}

impl Mul for &Metrics {
    type Output = Metrics;

    fn mul(self, rhs: Self) -> Self::Output {
        Metrics {
            ts: self.ts * rhs.ts,
            cycles: self.cycles * rhs.cycles,
            insn_count: self.insn_count * rhs.insn_count,
        }
    }
}

impl MulAssign for Metrics {
    fn mul_assign(&mut self, rhs: Self) {
        self.ts *= rhs.ts;
        self.cycles *= rhs.cycles;
        self.insn_count *= rhs.insn_count;
    }
}

impl PartialOrd for Metrics {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Metrics {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.ts.cmp(&other.ts)
    }
}

impl Sum for Metrics {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Metrics::constant(0), |acc, x| acc + x)
    }
}

impl<'a> Sum<&'a Metrics> for Metrics {
    fn sum<I: Iterator<Item = &'a Self>>(iter: I) -> Self {
        iter.fold(Metrics::constant(0), |acc, x| acc + *x)
    }
}

impl Display for Metrics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "(ts: {}, cycles: {}, insn_count: {})",
            self.ts, self.cycles, self.insn_count
        )
    }
}

struct InlineOrPtr<T> {
    bits: u64,
    _marker: PhantomData<T>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InlineOrPtrView<'a, T> {
    Inline(u64),
    Ptr(&'a T),
}

impl<T> InlineOrPtr<T> {
    const TAG_MASK: u64 = 1;

    fn new_heap(val: T) -> Self {
        let boxed = Box::new(val);
        let addr_u64 = (Box::into_raw(boxed) as usize) as u64;
        debug_assert_eq!(
            addr_u64 & Self::TAG_MASK,
            0,
            "Type alignment must be at least 2 for InlineOrPtr<T>"
        );
        InlineOrPtr {
            bits: addr_u64,
            _marker: PhantomData,
        }
    }

    fn new_inline(val: u64) -> Self {
        debug_assert_eq!(
            (val >> 63),
            0,
            "Inline value must fit in 63 bits for InlineOrPtr<T>"
        );
        InlineOrPtr {
            bits: (val << 1) | Self::TAG_MASK,
            _marker: PhantomData,
        }
    }

    fn is_inline(&self) -> bool {
        (self.bits & Self::TAG_MASK) != 0
    }

    fn as_enum<'a>(&self) -> InlineOrPtrView<'a, T> {
        if self.is_inline() {
            InlineOrPtrView::Inline(self.bits >> 1)
        } else {
            let ptr = (self.bits as usize) as *const T;
            unsafe { InlineOrPtrView::Ptr(&*ptr) }
        }
    }
}

impl<T> Drop for InlineOrPtr<T> {
    fn drop(&mut self) {
        if !self.is_inline() {
            let ptr = (self.bits as usize) as *mut T;
            unsafe {
                let _ = Box::from_raw(ptr);
            }
        }
    }
}

impl<T> Debug for InlineOrPtr<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.as_enum() {
            InlineOrPtrView::Inline(val) => write!(f, "Inline({})", val),
            InlineOrPtrView::Ptr(ptr) => write!(f, "Ptr({:p})", ptr),
        }
    }
}

impl<T: Clone> Clone for InlineOrPtr<T> {
    fn clone(&self) -> Self {
        if self.is_inline() {
            InlineOrPtr {
                bits: self.bits,
                _marker: PhantomData,
            }
        } else {
            let ptr = (self.bits as usize) as *const T;
            let cloned = unsafe { (*ptr).clone() };
            InlineOrPtr::new_heap(cloned)
        }
    }
}

impl<T: PartialEq> PartialEq for InlineOrPtr<T> {
    fn eq(&self, other: &Self) -> bool {
        self.as_enum() == other.as_enum()
    }
}

#[derive(Serialize, Deserialize)]
enum InlineOrPtrHelper<T> {
    Inline(u64),
    Ptr(T),
}

impl<T: Serialize> Serialize for InlineOrPtr<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.as_enum() {
            InlineOrPtrView::Inline(val) => {
                InlineOrPtrHelper::<T>::Inline(val).serialize(serializer)
            }
            InlineOrPtrView::Ptr(ptr) => InlineOrPtrHelper::Ptr(ptr).serialize(serializer),
        }
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for InlineOrPtr<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let helper = InlineOrPtrHelper::<T>::deserialize(deserializer)?;
        match helper {
            InlineOrPtrHelper::Inline(val) => Ok(InlineOrPtr::new_inline(val)),
            InlineOrPtrHelper::Ptr(ptr) => Ok(InlineOrPtr::new_heap(ptr)),
        }
    }
}

impl<T: Eq> Eq for InlineOrPtr<T> {}

impl<T: Hash> Hash for InlineOrPtr<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.is_inline().hash(state);
        match self.as_enum() {
            InlineOrPtrView::Inline(val) => val.hash(state),
            InlineOrPtrView::Ptr(ptr) => ptr.hash(state),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MetricsRange {
    // start is inclusive and end is exclusive
    pub start: Metrics,
    end: InlineOrPtr<Metrics>,
}

impl MetricsRange {
    // start must be less than end in all metrics
    pub fn new(start: Metrics, end: &Metrics) -> Self {
        assert!(end.ts >= start.ts);
        assert!(end.cycles >= start.cycles);
        assert!(end.insn_count >= start.insn_count);
        let ts_diff = end.ts - start.ts;
        let cycles_diff = end.cycles - start.cycles;
        let insn_diff = end.insn_count - start.insn_count;
        // store the difference in metrics if each field fits within 21 bits (21 * 3 = 63 bits total)
        // otherwise, store the end metrics on the heap
        if (ts_diff >> 21) == 0 && (cycles_diff >> 21) == 0 && (insn_diff >> 21) == 0 {
            let packed_diff = (ts_diff << 42) | (cycles_diff << 21) | insn_diff;
            MetricsRange {
                start,
                end: InlineOrPtr::new_inline(packed_diff),
            }
        } else {
            MetricsRange {
                start,
                end: InlineOrPtr::new_heap(*end),
            }
        }
    }

    #[inline]
    pub fn total_time(&self) -> u64 {
        match self.end.as_enum() {
            InlineOrPtrView::Inline(packed_diff) => {
                return (packed_diff >> 42) & 0x1FFFFF;
            }
            InlineOrPtrView::Ptr(end_metrics) => {
                return end_metrics.ts - self.start.ts;
            }
        }
    }

    #[inline]
    pub fn total_cycles(&self) -> u64 {
        match self.end.as_enum() {
            InlineOrPtrView::Inline(packed_diff) => {
                return (packed_diff >> 21) & 0x1FFFFF;
            }
            InlineOrPtrView::Ptr(end_metrics) => {
                return end_metrics.cycles - self.start.cycles;
            }
        }
    }

    #[inline]
    pub fn total_insn(&self) -> u64 {
        match self.end.as_enum() {
            InlineOrPtrView::Inline(packed_diff) => {
                return packed_diff & 0x1FFFFF;
            }
            InlineOrPtrView::Ptr(end_metrics) => {
                return end_metrics.insn_count - self.start.insn_count;
            }
        }
    }

    #[inline]
    pub fn end(&self) -> Metrics {
        let ts_diff = self.total_time();
        let cycles_diff = self.total_cycles();
        let insn_diff = self.total_insn();
        Metrics {
            ts: self.start.ts + ts_diff,
            cycles: self.start.cycles + cycles_diff,
            insn_count: self.start.insn_count + insn_diff,
        }
    }

    #[inline]
    pub fn includes_range(&self, other: &MetricsRange) -> bool {
        let other_end = other.end();
        let self_end = self.end();
        self.start.ts <= other.start.ts
            && other_end.ts <= self_end.ts
            && self.start.cycles <= other.start.cycles
            && other_end.cycles <= self_end.cycles
            && self.start.insn_count <= other.start.insn_count
            && other_end.insn_count <= self_end.insn_count
    }
}

impl Display for MetricsRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let end = self.end();
        write!(
            f,
            "MetricsRange {{ (ts: {}, cycles: {}, insn_count: {}) - (ts: {}, cycles: {}, insn_count: {}) }}",
            self.start.ts,
            self.start.cycles,
            self.start.insn_count,
            end.ts,
            end.cycles,
            end.insn_count
        )
    }
}

#[cfg(test)]
mod test {
    use crate::trace::Trace;

    use super::*;

    #[test]
    fn metric_range_inlined() {
        let zero_range = MetricsRange::new(
            Metrics {
                ts: 0,
                cycles: 1,
                insn_count: 2,
            },
            &Metrics {
                ts: 0,
                cycles: 1,
                insn_count: 2,
            },
        );
        let max_inlined = MetricsRange::new(
            Metrics {
                ts: 0,
                cycles: 1,
                insn_count: 2,
            },
            &Metrics {
                ts: 0x1fffff,
                cycles: 0x1ffff + 1,
                insn_count: 0x1ffff + 2,
            },
        );
        assert_eq!(zero_range.total_time(), 0);
        assert_eq!(zero_range.total_cycles(), 0);
        assert_eq!(zero_range.total_insn(), 0);
        assert_eq!(zero_range.end.bits, InlineOrPtr::<Metrics>::TAG_MASK);
        assert_eq!(max_inlined.total_time(), 0x1fffff);
        assert_eq!(max_inlined.total_cycles(), 0x1ffff);
        assert_eq!(max_inlined.total_insn(), 0x1ffff);
        let inlined_val = (0x1fffff << 42) | (0x1ffff << 21) | 0x1ffff;
        assert_eq!(
            max_inlined.end.bits,
            InlineOrPtr::<Metrics>::TAG_MASK | (inlined_val << 1)
        );
    }

    #[test]
    fn metric_range_boxed() {
        let min_boxed_end = Metrics {
            ts: 0x1fffff + 1,
            cycles: 0x1ffff + 2,
            insn_count: 0x1ffff + 3,
        };
        let min_boxed = MetricsRange::new(
            Metrics {
                ts: 0,
                cycles: 1,
                insn_count: 2,
            },
            &min_boxed_end,
        );
        assert_eq!(min_boxed.total_time(), 0x1fffff + 1);
        assert_eq!(min_boxed.total_cycles(), 0x1ffff + 1);
        assert_eq!(min_boxed.total_insn(), 0x1ffff + 1);
        assert!(!min_boxed.end.is_inline());
        assert_eq!(
            min_boxed.end.as_enum(),
            InlineOrPtrView::Ptr(&min_boxed_end)
        );
    }

    #[test]
    fn metric_range_serialize_round_trip() {
        let max_inlined = MetricsRange::new(
            Metrics {
                ts: 0,
                cycles: 1,
                insn_count: 2,
            },
            &Metrics {
                ts: 0x1fffff,
                cycles: 0x1ffff + 1,
                insn_count: 0x1ffff + 2,
            },
        );
        let min_boxed = MetricsRange::new(
            Metrics {
                ts: 0,
                cycles: 1,
                insn_count: 2,
            },
            &Metrics {
                ts: 0x1fffff + 1,
                cycles: 0x1ffff + 2,
                insn_count: 0x1ffff + 3,
            },
        );

        let config = Trace::bincode_config();

        let max_inlined_bytes = bincode_next::serde::encode_to_vec(&max_inlined, config).unwrap();
        let (max_inlined_deserialized, _) =
            bincode_next::serde::decode_from_slice(&max_inlined_bytes, config).unwrap();
        assert_eq!(max_inlined, max_inlined_deserialized);
        assert!(max_inlined_deserialized.end.is_inline());

        let min_boxed_bytes = bincode_next::serde::encode_to_vec(&min_boxed, config).unwrap();
        let (min_boxed_deserialized, _) =
            bincode_next::serde::decode_from_slice(&min_boxed_bytes, config).unwrap();
        assert_eq!(min_boxed, min_boxed_deserialized);
        assert!(!min_boxed_deserialized.end.is_inline());

        assert!(max_inlined_bytes.len() <= min_boxed_bytes.len());
    }
}
