// Read-only hosted diagnostics for at most four exact failing process incarnations.
// Never read command lines, environments, descriptor inventories, or target file contents.
import fs from 'node:fs';
import path from 'node:path';

const deadline = Date.now() + 5000;
const read = (filename) => {
  let fd;
  try {
    fd = fs.openSync(filename, 'r');
    const buffer = Buffer.alloc(65536);
    const length = fs.readSync(fd, buffer, 0, buffer.length, null);
    return { value: buffer.subarray(0, length).toString('utf8') };
  } catch (error) { return { error: { code: error.code, syscall: error.syscall } }; }
  finally { if (fd !== undefined) fs.closeSync(fd); }
};
const identity = (pid) => {
  const stat = read(`/proc/${pid}/stat`);
  const tail = stat.value?.slice(stat.value.lastIndexOf(')') + 2).trim().split(/\s+/);
  return { pid, start_ticks: tail?.[19], error: stat.error };
};
const sample = (pid) => {
  const root = `/proc/${pid}`;
  const stat = read(`${root}/stat`);
  const tail = stat.value?.slice(stat.value.lastIndexOf(')') + 2).trim().split(/\s+/);
  const status = read(`${root}/status`);
  const fields = {};
  const allowed = new Set(['Name', 'State', 'PPid', 'Threads', 'Uid', 'Gid', 'TracerPid',
    'NoNewPrivs', 'Seccomp', 'CapEff', 'CapPrm', 'CapAmb', 'Dumpable']);
  for (const line of status.value?.split('\n') ?? []) {
    const colon = line.indexOf(':');
    const key = line.slice(0, colon);
    if (allowed.has(key)) fields[key] = line.slice(colon + 1).trim().slice(0, 160);
  }
  let directory;
  try {
    const entry = fs.statSync(root);
    directory = { uid: entry.uid, gid: entry.gid, mode: entry.mode & 0o7777 };
  } catch (error) { directory = { error: error.code }; }
  let executable;
  try { executable = { basename: path.basename(fs.readlinkSync(`${root}/exe`)) }; }
  catch (error) { executable = { error: { code: error.code, syscall: error.syscall } }; }
  return { pid, start_ticks: tail?.[19], state: tail?.[0], parent: tail?.[1],
    stat_error: stat.error, status_error: status.error, fields, directory, executable };
};

console.log(JSON.stringify({ diagnostic_process_not_original_scanner: sample(process.pid), dumpability_note:
  'Other-process dumpability is not directly observed unless Dumpable is exposed; CoreDumping is not dumpability.' }));
const log = read(process.argv[2]);
if (log.error) {
  console.log(JSON.stringify({ log_error: log.error }));
} else {
  const seen = new Set();
  for (const match of log.value.matchAll(/operation="process_identity_incomplete" pid=(\d+) start_ticks=(\d+)/g)) {
    if (Date.now() >= deadline || seen.size >= 4) break;
    const [pid, expectedStart] = match.slice(1);
    const key = `${pid}:${expectedStart}`;
    if (seen.has(key)) continue;
    seen.add(key);
    const before = identity(Number(pid));
    if (before.start_ticks !== expectedStart || Date.now() >= deadline) {
      console.log(JSON.stringify({ expected_pid: pid, expected_start_ticks: expectedStart,
        same_incarnation: false, before, skipped_metadata: true }));
      continue;
    }
    const metadata = sample(Number(pid));
    const parentBestEffort = Date.now() < deadline && /^\d+$/.test(metadata.parent ?? '') && Number(metadata.parent) > 0
      ? sample(Number(metadata.parent)) : undefined;
    const after = identity(Number(pid));
    console.log(JSON.stringify({ expected_pid: pid, expected_start_ticks: expectedStart,
      same_incarnation: before.start_ticks === expectedStart && after.start_ticks === expectedStart,
      before, metadata, parent_best_effort_unverified: parentBestEffort, after }));
  }
  if (seen.size === 0) console.log(JSON.stringify({ no_identity_failure_in_capped_log: true }));
}
