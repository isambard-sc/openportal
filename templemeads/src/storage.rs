// SPDX-FileCopyrightText: © 2025 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::error::Error;

use crate::grammar::NamedType;

impl NamedType for StorageSize {
    fn type_name() -> &'static str {
        "StorageSize"
    }
}

impl NamedType for Vec<StorageSize> {
    fn type_name() -> &'static str {
        "Vec<StorageSize>"
    }
}

impl NamedType for StorageUsage {
    fn type_name() -> &'static str {
        "StorageUsage"
    }
}

impl NamedType for Vec<StorageUsage> {
    fn type_name() -> &'static str {
        "Vec<StorageUsage>"
    }
}

impl NamedType for QuotaLimit {
    fn type_name() -> &'static str {
        "QuotaLimit"
    }
}

impl NamedType for Vec<QuotaLimit> {
    fn type_name() -> &'static str {
        "Vec<QuotaLimit>"
    }
}

impl NamedType for StorageQuota {
    fn type_name() -> &'static str {
        "StorageQuota"
    }
}

impl NamedType for Vec<StorageQuota> {
    fn type_name() -> &'static str {
        "Vec<StorageQuota>"
    }
}

impl NamedType for StorageVolume {
    fn type_name() -> &'static str {
        "StorageVolume"
    }
}

impl NamedType for Vec<StorageVolume> {
    fn type_name() -> &'static str {
        "Vec<StorageVolume>"
    }
}

impl NamedType for HashMap<StorageVolume, StorageQuota> {
    fn type_name() -> &'static str {
        "HashMap<StorageVolume, StorageQuota>"
    }
}

/// Represents a quantity of storage in bytes
#[derive(Copy, Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct StorageSize {
    bytes: u64,
}

impl std::ops::Add for StorageSize {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            bytes: self.bytes + other.bytes,
        }
    }
}

impl std::ops::AddAssign for StorageSize {
    fn add_assign(&mut self, other: Self) {
        self.bytes += other.bytes;
    }
}

impl std::ops::Sub for StorageSize {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        Self {
            bytes: self.bytes.saturating_sub(other.bytes),
        }
    }
}

impl std::ops::SubAssign for StorageSize {
    fn sub_assign(&mut self, other: Self) {
        self.bytes = self.bytes.saturating_sub(other.bytes);
    }
}

impl std::ops::Mul<u64> for StorageSize {
    type Output = Self;

    fn mul(self, rhs: u64) -> Self {
        Self {
            bytes: self.bytes * rhs,
        }
    }
}

impl std::ops::MulAssign<u64> for StorageSize {
    fn mul_assign(&mut self, rhs: u64) {
        self.bytes *= rhs;
    }
}

impl std::ops::Div<u64> for StorageSize {
    type Output = Self;

    fn div(self, rhs: u64) -> Self {
        Self {
            bytes: self.bytes / rhs,
        }
    }
}

impl std::ops::DivAssign<u64> for StorageSize {
    fn div_assign(&mut self, rhs: u64) {
        self.bytes /= rhs;
    }
}

impl std::iter::Sum for StorageSize {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::default(), |a, b| Self {
            bytes: a.bytes + b.bytes,
        })
    }
}

impl std::fmt::Display for StorageSize {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self.bytes {
            0..=1024 => write!(f, "{} B", self.bytes),
            1025..=1_048_576 => write!(f, "{:.2} KB", self.bytes as f64 / 1024.0),
            1_048_577..=1_073_741_824 => {
                write!(f, "{:.2} MB", self.bytes as f64 / 1_048_576.0)
            }
            1_073_741_825..=1_099_511_627_776 => {
                write!(f, "{:.2} GB", self.bytes as f64 / 1_073_741_824.0)
            }
            _ => write!(f, "{:.2} TB", self.bytes as f64 / 1_099_511_627_776.0),
        }
    }
}

impl StorageSize {
    pub fn from_bytes(bytes: u64) -> Self {
        Self { bytes }
    }

    pub fn as_bytes(&self) -> u64 {
        self.bytes
    }

    pub fn from_kilobytes(kb: u64) -> Self {
        Self { bytes: kb * 1024 }
    }

    pub fn as_kilobytes(&self) -> f64 {
        self.bytes as f64 / 1024.0
    }

    pub fn from_megabytes(mb: u64) -> Self {
        Self {
            bytes: mb * 1_048_576,
        }
    }

    pub fn as_megabytes(&self) -> f64 {
        self.bytes as f64 / 1_048_576.0
    }

    pub fn from_gigabytes(gb: u64) -> Self {
        Self {
            bytes: gb * 1_073_741_824,
        }
    }

    pub fn as_gigabytes(&self) -> f64 {
        self.bytes as f64 / 1_073_741_824.0
    }

    pub fn from_terabytes(tb: u64) -> Self {
        Self {
            bytes: tb * 1_099_511_627_776,
        }
    }

    pub fn as_terabytes(&self) -> f64 {
        self.bytes as f64 / 1_099_511_627_776.0
    }

    pub fn parse(quantity: &str) -> Result<Self, Error> {
        let quantity = quantity.trim().to_uppercase();

        // split into number and unit
        let (number_str, unit) = quantity
            .chars()
            .partition::<String, _>(|c| c.is_ascii_digit() || *c == '.');

        let number: f64 = number_str
            .parse()
            .with_context(|| format!("Failed to parse '{}' as a number", number_str))?;

        let bytes = match unit.as_str() {
            "B" => number,
            "KB" => number * 1024.0,
            "MB" => number * 1_048_576.0,
            "GB" => number * 1_073_741_824.0,
            "TB" => number * 1_099_511_627_776.0,
            "PB" => number * 1_125_899_906_842_624.0,
            "BYTES" => number,
            "KILOBYTES" => number * 1024.0,
            "MEGABYTES" => number * 1_048_576.0,
            "GIGABYTES" => number * 1_073_741_824.0,
            "TERABYTES" => number * 1_099_511_627_776.0,
            "PETABYTES" => number * 1_125_899_906_842_624.0,
            _ => return Err(Error::Parse(format!("Unknown unit '{}'", unit))),
        };

        Ok(Self {
            bytes: bytes as u64,
        })
    }
}

/// Represents the amount of storage currently used
#[derive(Copy, Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct StorageUsage {
    size: StorageSize,
}

impl StorageUsage {
    pub fn new(size: StorageSize) -> Self {
        Self { size }
    }

    pub fn into_size(self) -> StorageSize {
        self.size
    }
}

// Deref allows StorageUsage to automatically expose all StorageSize methods
impl std::ops::Deref for StorageUsage {
    type Target = StorageSize;

    fn deref(&self) -> &Self::Target {
        &self.size
    }
}

impl std::ops::DerefMut for StorageUsage {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.size
    }
}

// This allows creating StorageUsage from StorageSize
impl From<StorageSize> for StorageUsage {
    fn from(size: StorageSize) -> Self {
        Self { size }
    }
}

// This allows creating StorageUsage directly from bytes
impl From<u64> for StorageUsage {
    fn from(bytes: u64) -> Self {
        Self {
            size: StorageSize::from_bytes(bytes),
        }
    }
}

impl std::fmt::Display for StorageUsage {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.size)
    }
}

/// Represents the limit of a storage quota
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuotaLimit {
    /// A hard limit on storage size
    Limited(StorageSize),
    /// No limit on storage size
    Unlimited,
}

impl QuotaLimit {
    pub fn is_unlimited(&self) -> bool {
        matches!(self, QuotaLimit::Unlimited)
    }

    pub fn is_limited(&self) -> bool {
        matches!(self, QuotaLimit::Limited(_))
    }

    pub fn size(&self) -> Option<StorageSize> {
        match self {
            QuotaLimit::Limited(size) => Some(*size),
            QuotaLimit::Unlimited => None,
        }
    }
}

impl std::fmt::Display for QuotaLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            QuotaLimit::Limited(size) => write!(f, "{}", size),
            QuotaLimit::Unlimited => write!(f, "unlimited"),
        }
    }
}

impl From<StorageSize> for QuotaLimit {
    fn from(size: StorageSize) -> Self {
        QuotaLimit::Limited(size)
    }
}

/// Represents a storage quota with a limit and optional current usage
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageQuota {
    limit: QuotaLimit,
    usage: Option<StorageUsage>,
}

impl StorageQuota {
    pub fn limited(limit: StorageSize) -> Self {
        Self {
            limit: QuotaLimit::Limited(limit),
            usage: None,
        }
    }

    pub fn unlimited() -> Self {
        Self {
            limit: QuotaLimit::Unlimited,
            usage: None,
        }
    }

    pub fn with_usage(limit: QuotaLimit, usage: StorageUsage) -> Self {
        Self {
            limit,
            usage: Some(usage),
        }
    }

    pub fn limit(&self) -> &QuotaLimit {
        &self.limit
    }

    pub fn usage(&self) -> Option<StorageUsage> {
        self.usage
    }

    pub fn set_usage(&mut self, usage: StorageUsage) {
        self.usage = Some(usage);
    }

    pub fn is_unlimited(&self) -> bool {
        self.limit.is_unlimited()
    }

    pub fn is_over_quota(&self) -> bool {
        match (&self.limit, self.usage) {
            (QuotaLimit::Limited(limit), Some(usage)) => usage.as_bytes() > limit.as_bytes(),
            _ => false,
        }
    }

    pub fn percentage_used(&self) -> Option<f64> {
        match (&self.limit, self.usage) {
            (QuotaLimit::Limited(limit), Some(usage)) if limit.as_bytes() > 0 => {
                Some((usage.as_bytes() as f64 / limit.as_bytes() as f64) * 100.0)
            }
            _ => None,
        }
    }
}

impl std::fmt::Display for StorageQuota {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self.usage {
            Some(usage) => match &self.limit {
                QuotaLimit::Limited(limit) => {
                    if let Some(percentage) = self.percentage_used() {
                        write!(f, "{} / {} | {:.1}%", usage, limit, percentage)
                    } else {
                        write!(f, "{} / {}", usage, limit)
                    }
                }
                QuotaLimit::Unlimited => write!(f, "{} / unlimited", usage),
            },
            None => write!(f, "{} limit", self.limit),
        }
    }
}

/// Identifies a storage volume (e.g., "home", "scratch", "project")
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct StorageVolume {
    name: String,
}

impl StorageVolume {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl std::fmt::Display for StorageVolume {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}
