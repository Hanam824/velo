//! The segmented download engine: probes a URL, splits it into ranges, downloads each range into a part file and merges the parts.

pub mod planner;
