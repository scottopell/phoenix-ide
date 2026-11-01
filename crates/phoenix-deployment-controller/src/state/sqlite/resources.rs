use std::num::NonZeroUsize;

use rusqlite::{params, Connection, OptionalExtension};

use super::{malformed, required, ControllerStoreError, Result, Word};
use crate::resource::{
    CpuContentionBasisPoints, MemoryPressure, ResourceCapacity, ResourceContention, ResourcePolicy,
    ResourceSample,
};

pub(super) fn write_policy(
    connection: &Connection,
    id: &[u8],
    policy: ResourcePolicy,
) -> Result<()> {
    let (memory, storage, cpu, pressure) = policy.thresholds();
    let required = u64::try_from(policy.required_consecutive_samples().get())
        .map_err(|_| ControllerStoreError::Malformed("policy size exceeds u64"))?;
    connection.execute(
        "INSERT INTO resource_policies VALUES (?1,?2,?3,?4,?5,?6)",
        params![
            id,
            Word(required),
            Word(memory),
            Word(storage),
            cpu.get(),
            pressure_text(pressure)
        ],
    )?;
    Ok(())
}

pub(super) fn read_policy(connection: &Connection, id: &[u8]) -> Result<Option<ResourcePolicy>> {
    let row = connection.query_row("SELECT required_samples,minimum_memory,minimum_storage,maximum_cpu,maximum_pressure FROM resource_policies WHERE attempt_id=?1", [id], |row| {
        Ok((row.get::<_, Word>(0)?.0, row.get::<_, Word>(1)?.0, row.get::<_, Word>(2)?.0,
            row.get::<_, u16>(3)?, row.get::<_, String>(4)?))
    }).optional()?;
    row.map(|(count, memory, storage, cpu, pressure)| {
        let count = usize::try_from(count)
            .map_err(|_| ControllerStoreError::Malformed("policy size exceeds usize"))?;
        Ok(ResourcePolicy::new(
            required(NonZeroUsize::new(count), "zero policy window")?,
            memory,
            storage,
            parse_cpu(cpu)?,
            parse_pressure(&pressure)?,
        ))
    })
    .transpose()
}

pub(super) fn write_samples(
    connection: &Connection,
    id: &[u8],
    samples: &[ResourceSample],
) -> Result<()> {
    let mut statement =
        connection.prepare("INSERT INTO resource_samples VALUES (?1,?2,?3,?4,?5,?6,?7)")?;
    for (index, sample) in samples.iter().enumerate() {
        let ordinal = i64::try_from(index)
            .map_err(|_| ControllerStoreError::Malformed("sample ordinal overflow"))?;
        statement.execute(params![
            id,
            ordinal,
            Word(sample.sequence),
            Word(sample.capacity.available_memory_bytes),
            Word(sample.capacity.available_storage_bytes),
            sample.contention.cpu.get(),
            pressure_text(sample.contention.memory_pressure)
        ])?;
    }
    Ok(())
}

pub(super) fn read_samples(connection: &Connection, id: &[u8]) -> Result<Vec<ResourceSample>> {
    let mut statement = connection.prepare("SELECT ordinal,sequence,available_memory,available_storage,cpu,pressure FROM resource_samples WHERE attempt_id=?1 ORDER BY ordinal")?;
    let mut rows = statement.query([id])?;
    let mut samples = Vec::new();
    while let Some(row) = rows.next()? {
        let ordinal: i64 = row.get(0)?;
        if usize::try_from(ordinal).ok() != Some(samples.len()) {
            return malformed("noncontiguous resource sample ordinals");
        }
        samples.push(ResourceSample {
            sequence: row.get::<_, Word>(1)?.0,
            capacity: ResourceCapacity {
                available_memory_bytes: row.get::<_, Word>(2)?.0,
                available_storage_bytes: row.get::<_, Word>(3)?.0,
            },
            contention: ResourceContention {
                cpu: parse_cpu(row.get(4)?)?,
                memory_pressure: parse_pressure(&row.get::<_, String>(5)?)?,
            },
        });
    }
    Ok(samples)
}

fn parse_cpu(value: u16) -> Result<CpuContentionBasisPoints> {
    CpuContentionBasisPoints::new(value)
        .map_err(|_| ControllerStoreError::Malformed("CPU contention out of range"))
}
const fn pressure_text(value: MemoryPressure) -> &'static str {
    match value {
        MemoryPressure::Nominal => "nominal",
        MemoryPressure::Warning => "warning",
        MemoryPressure::Critical => "critical",
    }
}
fn parse_pressure(value: &str) -> Result<MemoryPressure> {
    match value {
        "nominal" => Ok(MemoryPressure::Nominal),
        "warning" => Ok(MemoryPressure::Warning),
        "critical" => Ok(MemoryPressure::Critical),
        _ => malformed("unknown memory pressure"),
    }
}
