import { beforeEach } from 'vitest'
import {
  buildCpuProfileRecord,
  createCpuProfileWriter,
  isCpuProfilingEnabled,
  startCpuProfileWindow,
} from './testCpuProfile'

const writer = createCpuProfileWriter()

if (isCpuProfilingEnabled() && writer) {
  beforeEach((context) => {
    const window = startCpuProfileWindow()

    context.onTestFinished((finishedContext) => {
      writer.appendLine(buildCpuProfileRecord(finishedContext.task, window))
    })
  })
}
