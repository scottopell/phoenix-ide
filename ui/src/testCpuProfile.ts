import { appendFileSync, mkdirSync } from 'node:fs'
import { join } from 'node:path'
import { performance } from 'node:perf_hooks'
import type { TestContext } from 'vitest'

type CpuUsage = NodeJS.CpuUsage

type TestTaskLike = TestContext['task']

export interface TestCpuProfileRecord {
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
  wall_time_ms: number
  cpu_user_us: number
  cpu_system_us: number
}

export interface TestCpuProfileWindow {
  cpuStart: CpuUsage
  wallStartMs: number
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
  }
}

export function buildCpuProfileRecord(
  task: TestTaskLike,
  window: TestCpuProfileWindow,
): TestCpuProfileRecord {
  const cpuUsage = process.cpuUsage(window.cpuStart)
  const suite = task.suite

  return {
    provenance: 'windowed_process',
    pid: process.pid,
    file: task.file.filepath,
    project_name: task.file.projectName || null,
    test_id: task.id,
    suite_id: suite?.id ?? null,
    test_name: task.name,
    suite_name: suite?.fullTestName ?? null,
    full_name: task.fullName,
    full_test_name: task.fullTestName ?? task.fullName,
    status: task.result?.state ?? null,
    concurrent: Boolean(task.concurrent),
    wall_time_ms: Math.max(0, performance.now() - window.wallStartMs),
    cpu_user_us: cpuUsage.user,
    cpu_system_us: cpuUsage.system,
  }
}
