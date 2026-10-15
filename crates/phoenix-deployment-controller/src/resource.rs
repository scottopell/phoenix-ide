use std::num::NonZeroUsize;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryPressure {
    Nominal,
    Warning,
    Critical,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CpuContentionBasisPoints(u16);

impl CpuContentionBasisPoints {
    /// # Errors
    /// Returns [`InvalidCpuContention`] when `value` exceeds 10,000 basis points.
    pub fn new(value: u16) -> Result<Self, InvalidCpuContention> {
        if value > 10_000 {
            return Err(InvalidCpuContention);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidCpuContention;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResourceCapacity {
    pub available_memory_bytes: u64,
    pub available_storage_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResourceContention {
    pub cpu: CpuContentionBasisPoints,
    pub memory_pressure: MemoryPressure,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResourceSample {
    pub sequence: u64,
    pub capacity: ResourceCapacity,
    pub contention: ResourceContention,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResourcePolicy {
    required_consecutive_samples: NonZeroUsize,
    minimum_available_memory_bytes: u64,
    minimum_available_storage_bytes: u64,
    maximum_cpu_contention: CpuContentionBasisPoints,
    maximum_memory_pressure: MemoryPressure,
}

impl ResourcePolicy {
    #[must_use]
    pub const fn new(
        required_consecutive_samples: NonZeroUsize,
        minimum_available_memory_bytes: u64,
        minimum_available_storage_bytes: u64,
        maximum_cpu_contention: CpuContentionBasisPoints,
        maximum_memory_pressure: MemoryPressure,
    ) -> Self {
        Self {
            required_consecutive_samples,
            minimum_available_memory_bytes,
            minimum_available_storage_bytes,
            maximum_cpu_contention,
            maximum_memory_pressure,
        }
    }

    #[must_use]
    pub const fn required_consecutive_samples(self) -> NonZeroUsize {
        self.required_consecutive_samples
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceBlock {
    MemoryCapacity,
    StorageCapacity,
    CpuContention,
    MemoryPressure,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SustainedResourceEvidence {
    samples: Vec<ResourceSample>,
}

impl SustainedResourceEvidence {
    #[must_use]
    pub fn samples(&self) -> &[ResourceSample] {
        &self.samples
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResourceAssessment {
    NeedMoreSamples { observed: usize, required: usize },
    Admitted(SustainedResourceEvidence),
    Blocked(ResourceBlock),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NonIncreasingSampleSequence;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceWindow {
    policy: ResourcePolicy,
    samples: Vec<ResourceSample>,
}

impl ResourceWindow {
    #[must_use]
    pub const fn new(policy: ResourcePolicy) -> Self {
        Self {
            policy,
            samples: Vec::new(),
        }
    }

    /// # Errors
    /// Returns [`NonIncreasingSampleSequence`] when the sample does not follow the previously recorded sequence.
    pub fn record(
        &mut self,
        sample: ResourceSample,
    ) -> Result<ResourceAssessment, NonIncreasingSampleSequence> {
        if self
            .samples
            .last()
            .is_some_and(|previous| sample.sequence <= previous.sequence)
        {
            return Err(NonIncreasingSampleSequence);
        }

        self.samples.push(sample);
        let required = self.policy.required_consecutive_samples.get();
        if self.samples.len() > required {
            self.samples.remove(0);
        }

        Ok(self.assess())
    }

    #[must_use]
    pub fn assess(&self) -> ResourceAssessment {
        let required = self.policy.required_consecutive_samples.get();
        if self.samples.len() < required {
            return ResourceAssessment::NeedMoreSamples {
                observed: self.samples.len(),
                required,
            };
        }

        if self.samples.iter().any(|sample| {
            sample.capacity.available_memory_bytes < self.policy.minimum_available_memory_bytes
        }) {
            return ResourceAssessment::Blocked(ResourceBlock::MemoryCapacity);
        }
        if self.samples.iter().any(|sample| {
            sample.capacity.available_storage_bytes < self.policy.minimum_available_storage_bytes
        }) {
            return ResourceAssessment::Blocked(ResourceBlock::StorageCapacity);
        }
        if self
            .samples
            .iter()
            .all(|sample| sample.contention.cpu.get() > self.policy.maximum_cpu_contention.get())
        {
            return ResourceAssessment::Blocked(ResourceBlock::CpuContention);
        }
        if self.samples.iter().all(|sample| {
            pressure_rank(sample.contention.memory_pressure)
                > pressure_rank(self.policy.maximum_memory_pressure)
        }) {
            return ResourceAssessment::Blocked(ResourceBlock::MemoryPressure);
        }

        ResourceAssessment::Admitted(SustainedResourceEvidence {
            samples: self.samples.clone(),
        })
    }
}

const fn pressure_rank(pressure: MemoryPressure) -> u8 {
    match pressure {
        MemoryPressure::Nominal => 0,
        MemoryPressure::Warning => 1,
        MemoryPressure::Critical => 2,
    }
}
