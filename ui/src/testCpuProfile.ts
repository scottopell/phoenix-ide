import { appendFileSync, mkdirSync } from 'node:fs'
import { join, relative, sep } from 'node:path'
import { performance } from 'node:perf_hooks'
import type { TestContext } from 'vitest'

type CpuUsage = NodeJS.CpuUsage

type TestTaskLike = TestContext['task']

export interface TestCpuProfileRecord {
  schema_version: 1
  provenance: 'windowed_process'
  pid: number
  file: string
  project_name: string | null
  test_id: string
  suite_id: string | null
  test_name: string
  suite_name: string | null
  full_name: string
  full_test_name: string
  status: string | null
  concurrent: boolean
  started_unix_ns: number
  wall_time_ms: number
  cpu_user_us: number
  cpu_system_us: number
}

export interface TestCpuProfileWindow {
  cpuStart: CpuUsage
  wallStartMs: number
  startedUnixNs: bigint
}

export interface TestCpuProfileWriter {
  appendLine(record: TestCpuProfileRecord): void
}

const profileDir = process.env['PHOENIX_CHECK_PROFILE_DIR']

export function currentWorkerId(): string {
  return process.env['VITEST_POOL_ID'] ?? process.env['VITEST_WORKER_ID'] ?? 'worker-unknown'
}

export function isCpuProfilingEnabled(): boolean {
  return Boolean(profileDir)
}

export function createCpuProfileWriter(dir: string = profileDir ?? ''): TestCpuProfileWriter | null {
  if (!dir) {
    return null
  }

  mkdirSync(dir, { recursive: true })
  const path = join(dir, `vitest-cpu-${process.pid}-${currentWorkerId()}.jsonl`)

  return {
    appendLine(record) {
      appendFileSync(path, `${JSON.stringify(record)}\n`, 'utf8')
    },
  }
}

export function startCpuProfileWindow(): TestCpuProfileWindow {
  return {
    cpuStart: process.cpuUsage(),
    wallStartMs: performance.now(),
    startedUnixNs: BigInt(Date.now()) * 1_000_000n,
  }
}

function stableTestFile(filepath: string): string {
  return relative(process.cwd(), filepath).split(sep).join('/')
}


export function buildCpuProfileRecord(
  task: TestTaskLike,
  window: TestCpuProfileWindow,
): TestCpuProfileRecord {
  const cpuUsage = process.cpuUsage(window.cpuStart)
  const suite = task.suite
  const file = stableTestFile(task.file.filepath)
  const fullTestName = task.fullTestName ?? task.name

  return {
    provenance: 'windowed_process',
    schema_version: 1,
    pid: process.pid,
    file,
    project_name: task.file.projectName || null,
    test_id: task.id,
    suite_id: suite?.id ?? null,
    test_name: task.name,
    suite_name: suite?.fullTestName ?? null,
    full_name: `${file} > ${fullTestName}`,
    full_test_name: fullTestName,
    status: task.result?.state ?? null,
    concurrent: Boolean(task.concurrent),
    started_unix_ns: Number(window.startedUnixNs),
    wall_time_ms: Math.max(0, performance.now() - window.wallStartMs),
    cpu_user_us: cpuUsage.user,
    cpu_system_us: cpuUsage.system,
  }
}
