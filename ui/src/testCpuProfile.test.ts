import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, describe, expect, it, vi } from 'vitest'

import {
  buildCpuProfileRecord,
  createCpuProfileWriter,
  startCpuProfileWindow,
  type TestCpuProfileRecord,
} from './testCpuProfile'

describe('testCpuProfile', () => {
  afterEach(() => {
    vi.unstubAllEnvs()
    vi.restoreAllMocks()
  })

  it('writes newline-delimited JSON records to a per-worker file', () => {
    const dir = mkdtempSync(join(tmpdir(), 'vitest-cpu-profile-'))

    try {
      vi.stubEnv('VITEST_POOL_ID', '7')
      const writer = createCpuProfileWriter(dir)
      expect(writer).not.toBeNull()

      const record: TestCpuProfileRecord = {
        schema_version: 1,
        provenance: 'windowed_process',
        pid: 123,
        file: '/tmp/example.test.ts',
        project_name: null,
        test_id: 'id-1',
        suite_id: 'suite-1',
        test_name: 'records cpu',
        suite_name: 'suite',
        full_name: '/tmp/example.test.ts > suite > records cpu',
        full_test_name: 'suite > records cpu',
        status: 'pass',
        concurrent: false,
        started_unix_ns: 1_000_000_000,
        wall_time_ms: 12.5,
        cpu_user_us: 10,
        cpu_system_us: 3,
      }

      writer?.appendLine(record)
      writer?.appendLine({ ...record, test_id: 'id-2', test_name: 'second test' })

      const path = join(dir, `vitest-cpu-${process.pid}-7.jsonl`)
      const lines = readFileSync(path, 'utf8').trimEnd().split('\n')

      expect(lines).toHaveLength(2)
      expect(lines.map((line) => JSON.parse(line))).toEqual([
        record,
        { ...record, test_id: 'id-2', test_name: 'second test' },
      ])
    }
    finally {
      rmSync(dir, { recursive: true, force: true })
    }
  })

  it('builds a record from the vitest task identity and result state', () => {
    vi.spyOn(process, 'cpuUsage')
      .mockReturnValueOnce({ user: 11, system: 12 })
      .mockReturnValueOnce({ user: 2500, system: 500 })

    const window = startCpuProfileWindow()
    const task = {
      id: 'file_0_1',
      name: 'captures profile',
      fullName: `${join(process.cwd(), 'src/example.test.ts')} > outer suite > captures profile`,
      fullTestName: 'outer suite > captures profile',
      concurrent: false,
      file: {
        filepath: join(process.cwd(), 'src/example.test.ts'),
        projectName: 'ui',
      },
      suite: {
        id: 'file_0',
        fullTestName: 'outer suite',
      },
      result: {
        state: 'pass',
      },
    } as const

    const record = buildCpuProfileRecord(task as never, window)

    expect(record).toMatchObject({
      schema_version: 1,
      provenance: 'windowed_process',
      pid: process.pid,
      file: 'src/example.test.ts',
      project_name: 'ui',
      test_id: 'file_0_1',
      suite_id: 'file_0',
      test_name: 'captures profile',
      suite_name: 'outer suite',
      full_name: 'src/example.test.ts > outer suite > captures profile',
      full_test_name: 'outer suite > captures profile',
      status: 'pass',
      concurrent: false,
      cpu_user_us: 2500,
      cpu_system_us: 500,
    })
    expect(record.wall_time_ms).toBeGreaterThanOrEqual(0)
    expect(record.started_unix_ns).toBeGreaterThan(1_000_000_000_000_000_000)
  })
})
