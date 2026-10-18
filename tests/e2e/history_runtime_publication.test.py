#!/usr/bin/env python3
"""Synthetic filesystem tests only: no SQL, HTTP, browser, or app execution."""
import json
from pathlib import Path
import re
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).parent))
import history_runtime_publication as publication


class SafePublicationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()

    def write(self, name, value):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(value))
        return path

    def test_only_explicit_files_and_typed_fields_can_cross_upload_boundary(self):
        secret = 'UNIT_ONLY_private_cookie_password_DO_NOT_PUBLISH'
        self.write('result.json', {'status': 'failed', 'error': secret, 'runtime_executed': True,
                                  'joint_telemetry': True, 'telemetry_evidence_class': 'stock_natural',
                                  'full_acceptance': True, 'overhead_clearance': True})
        self.write('identity.json', {'app_source': 'a' * 40, 'executable_sha256': 'b' * 64,
                                    'password': secret, 'authkey': secret,
                                    'evidence_class': 'instrumented_cfg_test_library_not_shipping_runtime',
                                    'ui_artifacts': {'stock_natural': {'manifest_sha256': 'c' * 64, secret: secret}}})
        self.write('ordinary/authentication.json', {'auth_required': True, 'parent_authenticated': True,
                                                   'Cookie': secret, 'password': secret})
        self.write('ordinary/server-completion.json', {'status': 'success', 'returncode': 0,
                   'socket_removed': True, 'owned_home_removed': True, 'owned_home': secret})
        self.write('ordinary/lifecycle-failures.json', {'primary': {'message': secret}, 'cleanup': []})
        self.write('ordinary/browser-result.json', {'status': 'passed_assertions', 'checks': [secret],
                                                   'authentication': {'cookie': secret}})
        uid = '12345678-1234-4234-8234-123456789abc'
        rid = 'history-ordinary-' + 'a' * 16
        before = [{'name': name, 'value': index + .125} for index, name in enumerate(publication.CDP_METRICS)]
        after = [{'name': name, 'value': index + .625} for index, name in enumerate(publication.CDP_METRICS)]
        marks = {name: index + .25 for index, name in enumerate(publication.MARKS)}
        marks['shell'] = None
        report = {'open_id': uid, 'frontend_source': 'a' * 40, 'route': 'ordinary', 'outcome': 'ready',
                  'trigger': 'navigation', 'presentation_evidence': 'commit_frame_estimate',
                  'marks': marks | {secret: secret}, 'total_ms': 19.25,
                  'breadcrumb_ready_minus_meaningful_ms': -.5,
                  'correlations': [{'kind': 'request', 'id': uid, secret: secret}],
                  'password': secret, 'platform': secret}
        trial = {'route': 'ordinary', 'index': 0, 'sampled': True, 'warmup': False, 'posts': 1,
                 'payloadBytes': 128, 'failure': secret, 'metric_reset': False, 'context_closed': True,
                 'navigation_to_visible_readiness_ms': 123.625, 'admitted_open_id': uid,
                 'rawMetrics': {'before': before, 'after': after, secret: {'password': secret}},
                 'metrics': {'TaskDuration': 9.125, 'JSHeapUsedSize': 4096, secret: secret},
                 'observer': {'inspection_count': 19, 'sample_rate': .25, secret: secret},
                 'reports': [report], 'producer_events': [
                     {'version': 1, 'kind': 'admission', 'route': 'ordinary', 'sampled': True, 'open_id': uid, secret: secret},
                     {'version': 1, 'kind': 'mark', 'open_id': uid, 'mark': 'shell', 'ms': .25, 'password': secret}],
                 'response_statuses': [204], 'capture_errors': [], secret: {'cookies': secret}}
        self.write('ordinary/telemetry/telemetry-browser-result.json', {
            'runId': rid, 'identity': {'serverInstanceId': uid, 'appSource': 'a' * 40,
                                     'frontendSource': 'a' * 40, 'authkey': secret},
            'trials': [trial, {'index': 1, 'route': 'global', 'sampled': False, 'warmup': True,
                              'metric_reset': True, 'producer_events': [
                                  {'kind': 'admission', 'open_id': None, 'sampled': False, 'route': 'global'}]}],
            'terminal': {'trials': [trial]}, 'lifecycle': {'trials': [trial]},
            'journeys': [{'open_id': uid, 'correlations': report['correlations'], 'expected': {
                'browser.total_ms': 19.25, 'browser.meaningful_transcript_ms': None,
                'browser.frontend_source': 'a' * 40, 'server.instance_id': uid, secret: secret}}]})
        self.write('ordinary/telemetry-delivery.json', {
            'run_id': rid, 'evidence_class': 'stock_natural', 'normal_drain': True,
            'collector_acceptances': [{'schema_version': 1, 'open_id': uid, 'server_instance_id': uid,
                                      'backend_source': 'a' * 40, 'status': 204, 'authkey': secret}],
            'exported': [{'open_id': uid, 'trace_id': 'd' * 32, 'outcome': 'ready', secret: secret}]})
        for name in ('ordinary/startup.json', 'ordinary/credential-identity.json',
                     'ordinary/authkey', 'ordinary/fixture.db', 'ordinary/storage-state.json',
                     'ordinary/telemetry/batch-001.pb', 'ordinary/browser.log'):
            self.write(name, {'password': secret})
        value = publication.project(self.root)
        self.assertNotIn(secret, json.dumps(value))
        self.assertEqual(value['identity']['app_source'], 'a' * 40)
        self.assertFalse(value['full_acceptance'])
        self.assertFalse(value['overhead_clearance'])
        case = value['cases']['ordinary']
        self.assertTrue(case['authentication']['parent_authenticated'])
        self.assertEqual(case['server_completion']['returncode'], 0)
        self.assertTrue(case['lifecycle_failure_present'])
        self.assertEqual(case['browser_check_count'], 1)
        self.assertEqual(case['natural_trials'][0]['posts'], 1)
        self.assertTrue(case['natural_trials'][0]['failure_present'])
        saved = case['natural_trials'][0]
        self.assertEqual(saved['rawMetrics']['before']['items'], before)
        self.assertEqual(saved['rawMetrics']['after']['items'], after)
        self.assertEqual(saved['metrics'], {'TaskDuration': 9.125, 'JSHeapUsedSize': 4096})
        self.assertEqual(saved['navigation_to_visible_readiness_ms'], 123.625)
        self.assertEqual(saved['reports']['items'][0]['marks'], marks)
        self.assertEqual(saved['reports']['items'][0]['total_ms'], 19.25)
        self.assertEqual(saved['reports']['items'][0]['breadcrumb_ready_minus_meaningful_ms'], -.5)
        self.assertEqual(saved['producer_events']['items'][0]['open_id'], uid)
        self.assertEqual(saved['producer_events']['items'][1]['ms'], .25)
        self.assertEqual([t['index'] for t in case['natural_trials']], [0, 1])
        self.assertTrue(case['natural_trials'][1]['metric_reset'])
        self.assertTrue(case['natural_trials'][1]['warmup'])
        self.assertFalse(case['natural_trials'][1]['sampled'])
        self.assertIsNone(case['natural_trials'][1]['producer_events']['items'][0]['open_id'])
        for stage in ('terminal', 'lifecycle'):
            self.assertEqual(case['stages'][stage]['items'][0]['rawMetrics'], saved['rawMetrics'])
        self.assertEqual(case['journeys']['items'][0]['expected']['browser.total_ms'], 19.25)
        self.assertIsNone(case['journeys']['items'][0]['expected']['browser.meaningful_transcript_ms'])
        self.assertEqual(case['identity']['serverInstanceId'], uid)
        self.assertEqual(case['identity']['frontendSource'], 'a' * 40)
        self.assertEqual(case['collector_acceptances']['items'][0]['open_id'], uid)
        self.assertEqual(case['exported']['items'][0]['trace_id'], 'd' * 32)
        self.assertEqual(case['run_correlation']['runId'], rid)
        self.assertEqual(case['delivery_correlation']['run_id'], rid)
        self.assertEqual(value['identity']['evidence_class'], 'instrumented_cfg_test_library_not_shipping_runtime')
        self.assertEqual(value['ui_artifacts']['stock_natural']['manifest_sha256'], 'c' * 64)
        self.assertEqual(case['publication_gates']['long_tasks'], 'not_emitted_by_pinned_driver')

    def diagnostic(self, line=530, phase='module_checks'):
        return {'schema_version': 1, 'resolution': 'verified_assertion_site', 'phase': phase,
                'error_code': 'ERR_ASSERTION', 'module_basename': 'qualify-view-telemetry.mjs',
                'module_sha256': publication.ASSERTION_MODULES['qualify-view-telemetry.mjs'],
                'line': line, 'column': 10,
                'assertion_site_id': f'f16-qualify-view-telemetry-L{line}-C10'}

    def test_exact_source_assertion_projection_keeps_primary_separate_from_secondary(self):
        primary = self.diagnostic()
        secondary = self.diagnostic(531, 'context_cleanup')
        secret = 'UNIT_ONLY_cookie_https://private.invalid/?token=SECRET'
        path = 'ordinary/joint-telemetry-dependency-manifest.json'
        self.write(path, {'failure_phase': 'module_checks', 'primary_failure': secret,
            'primary_diagnostic': primary | {'stack': secret, 'message': secret, 'url': secret},
            'secondary_diagnostics': [secondary | {'message': secret}], 'cleanup_failures': [secret]})
        result = publication.project(self.root)['cases']['ordinary']['diagnostics']['joint_dependency_manifest']
        self.assertEqual(result['primary_diagnostic'], primary)
        self.assertEqual(result['primary_diagnostic_boundary'], 'present')
        self.assertEqual(result['secondary_diagnostics']['items'], [secondary])
        self.assertNotIn(secret, json.dumps(result))
        self.write(path, {'primary_diagnostic': primary, 'secondary_diagnostics': [secondary] * 4})
        bounded = publication.project(self.root)['cases']['ordinary']['diagnostics']['joint_dependency_manifest']
        self.assertEqual(bounded['primary_diagnostic'], primary)
        self.assertEqual(len(bounded['secondary_diagnostics']['items']), 2)
        self.assertTrue(bounded['secondary_diagnostics']['overflow'])
        self.assertEqual(publication.joint_diagnostics({})['primary_diagnostic_boundary'], 'missing_field')
        self.assertIsNone(publication.joint_diagnostics({'primary_diagnostic': None})['primary_diagnostic'])

    def test_source_diagnostic_unknown_fallback_rejects_malicious_or_unverified_fields(self):
        secret = 'UNIT_ONLY_SECRET'
        for change in ({'module_basename': '../' + secret}, {'module_sha256': '0' * 64},
                       {'line': 529}, {'column': 11}, {'line': True}, {'schema_version': True},
                       {'assertion_site_id': secret}, {'phase': secret}, {'error_code': secret},
                       {'resolution': secret}, {'line': -1}, {'line': 99999}, {'column': secret}):
            result = publication.assertion_diagnostic(self.diagnostic() | change)
            self.assertEqual(result['resolution'], 'unknown', change)
            self.assertNotIn('module_basename', result)
            self.assertNotIn(secret, json.dumps(result))
        self.assertEqual(publication.assertion_diagnostic(self.diagnostic(531, 'context_cleanup'))['resolution'], 'unknown')
        from unittest.mock import patch
        with patch.object(Path, 'read_bytes', return_value=b'UNIT_ONLY_CHANGED_SOURCE'):
            self.assertEqual(publication.assertion_diagnostic(self.diagnostic())['resolution'], 'unknown')
        for decoy in (b'// assert.equal(1, 2)', b'/* assert.equal(1, 2) */', b'"assert.equal(1, 2)";'):
            with patch.object(Path, 'read_bytes', return_value=b'\n' * 529 + b'  ' + decoy):
                self.assertEqual(publication.assertion_diagnostic(self.diagnostic())['resolution'], 'unknown')

    def test_real_node_assertion_stacks_map_to_independently_fixed_publisher_sites(self):
        import subprocess
        root = Path(__file__).resolve().parents[2]
        script = r'''
          import {fileURLToPath} from 'node:url';
          import {readFileSync} from 'node:fs';
          import {runInNewContext} from 'node:vm';
          import assert from 'node:assert/strict';
          import {assertBackgroundReport} from './ui/scripts/qualify-view-telemetry.mjs';
          import {prepareJointFailureDiagnostic,TELEMETRY_MODULE_SHA256,TELEMETRY_DIAGNOSTICS_SHA256} from './ui/scripts/history-joint-telemetry.mjs';
          const modulePath=fileURLToPath(new URL('./ui/scripts/qualify-view-telemetry.mjs',import.meta.url));
          const helper=fileURLToPath(new URL('./ui/scripts/view-telemetry-diagnostics.mjs',import.meta.url));
          const diagnose=prepareJointFailureDiagnostic({modulePath,dependencies:{[modulePath]:TELEMETRY_MODULE_SHA256,[helper]:TELEMETRY_DIAGNOSTICS_SHA256}});
          const output=[];
          for (const path of [modulePath,helper]) readFileSync(path,'utf8').split('\n').forEach((line,index)=>{
            for(const match of line.matchAll(/\bassert\.(\w+)\(/g)) {
              const args={equal:'1,2',notEqual:'1,1',deepEqual:'1,2',ok:'false',match:"'unit',/never/"}[match[1]];
              const run=runInNewContext('(() => {'+'\n'.repeat(index)+' '.repeat(match.index)
                +`assert.${match[1]}(${args}, 'UNIT_ONLY_NOT_RUNTIME');\n})`,{assert},{filename:path});
              let error; try {run();} catch(thrown) {error=thrown;}
              assert.ok(error instanceof Error);
              assert.equal(error.code,'ERR_ASSERTION');
              assert.ok(error.stack.includes(`    at ${path}:${index+1}:${match.index+8}`));
              output.push(diagnose(error,'module_checks'));
            }
          });
          try {assertBackgroundReport({},{});} catch(error) {output.push(diagnose(error,'module_checks'));}
          console.log(JSON.stringify(output));
        '''
        result = subprocess.run(['node', '--input-type=module', '-e', script], cwd=root,
                                check=True, capture_output=True, text=True, timeout=10)
        diagnostics = json.loads(result.stdout)
        self.assertEqual(len(diagnostics), 167)
        self.assertEqual(sum(map(len, publication.ASSERTION_SITES.values())), 166)
        self.assertEqual(diagnostics[-1], self.diagnostic())
        for diagnostic in diagnostics:
            self.assertEqual(diagnostic['resolution'], 'verified_assertion_site')
            self.assertEqual(publication.assertion_diagnostic(diagnostic), diagnostic)

    def test_fixed_inventory_and_phases_are_pinned_to_actual_producers(self):
        scripts = Path(__file__).resolve().parents[2] / 'ui/scripts'
        browser = (scripts / 'qualify-history-opening-realserver.mjs').read_text()
        checks = set(re.findall(r"checked\('([^']+)'", browser))
        checks.update(re.findall(r"sameAnchor\(before, '([^']+)'", browser))
        self.assertEqual(set(publication.HISTORY_CHECKS), checks)
        self.assertEqual(len(publication.HISTORY_CHECKS), len(checks))
        joint = (scripts / 'history-joint-telemetry.mjs').read_text().split('export async function runJointTelemetryStage', 1)[1]
        self.assertEqual(set(publication.JOINT_PHASES), set(re.findall(r"phase = '([^']+)'", joint)))
        cleanup = set(re.findall(r"retainCleanupFailure\(result, '([^']+)'", browser))
        self.assertEqual(set(publication.BROWSER_CLEANUP_STAGES), cleanup)
        producer = (Path(__file__).parent / 'history_opening_browser.py').read_text()
        stages = set(re.findall(r'remember_(?:primary|cleanup)\("([^"]+)"', producer))
        stages.update(re.findall(r'\), "(receiver|server|preview|browser)"\) as', producer))
        # receiver_context is passed directly rather than wrapped in owned_process().
        stages.add('receiver')
        self.assertEqual(set(publication.LIFECYCLE_STAGES), stages)

    def test_failed_nine_checks_and_known_phase_survive_secret_mutations(self):
        previous = None
        for secret in ('UNIT_ONLY_cookie_abc', 'https://private.invalid/path?token=UNIT_ONLY_def'):
            nested = {'message': secret, 'headers': {'Authorization': secret}, 'path': secret,
                      'url': secret, 'database': secret, 'raw_otlp': secret}
            self.write('result.json', {'status': 'failed', 'error': secret, 'aggregation_errors': [nested]})
            self.write('ordinary/browser-result.json', {'status': 'failed',
                'checks': list(publication.HISTORY_CHECKS[:9]), 'error': secret,
                'cleanup_failures': [{'stage': 'browser_cleanup', 'error': secret, 'truncated': False,
                                      'network': nested}], 'cleanup_overflow': False,
                'application_page_errors': [nested], 'cdp_capture_errors': [secret], 'network': nested})
            self.write('ordinary/joint-telemetry-dependency-manifest.json', {
                'failure_phase': 'module_checks', 'context_acquired': True,
                'primary_failure': secret, 'authentication_response_cleanup_failure': secret,
                'cleanup_failures': [secret], 'before': nested, 'after': nested})
            self.write('ordinary/lifecycle-failures.json', {'primary': {'stage': 'browser',
                'type': secret, 'message': secret, 'message_truncated': False},
                'cleanup': [{'stage': 'server_control_stop', 'message': secret, 'network': nested}],
                'cleanup_overflow': False})
            value = publication.project(self.root)
            serialized = json.dumps(value)
            self.assertNotIn(secret, serialized)
            self.assertNotIn('Authorization', serialized)
            self.assertNotIn('raw_otlp', serialized)
            if previous is not None:
                self.assertEqual(value, previous)
            previous = value
            diag = value['cases']['ordinary']['diagnostics']
            self.assertEqual(diag['browser']['checks']['items'], [
                {'name': name, 'passed': True} for name in publication.HISTORY_CHECKS[:9]])
            self.assertEqual(diag['joint_dependency_manifest']['failure_phase'], 'module_checks')
            self.assertTrue(diag['joint_dependency_manifest']['context_acquired'])
            self.assertTrue(diag['joint_dependency_manifest']['primary_failure_present'])
            self.assertEqual(diag['joint_dependency_manifest']['cleanup_failure_count'], 1)
            self.assertEqual(diag['lifecycle_failures']['primary']['stage'], 'browser')
            self.assertTrue(diag['lifecycle_failures']['primary']['message_present'])
            self.assertEqual(diag['browser']['cleanup_failures']['items'][0]['stage'], 'browser_cleanup')
        for phase in publication.JOINT_PHASES:
            self.assertEqual(publication.joint_diagnostics({'failure_phase': phase})['failure_phase'], phase)
        unknown = publication.joint_diagnostics({'failure_phase': 'UNIT_ONLY_secret',
                                                 'primary_failure': 'module_checks'})
        self.assertNotIn('failure_phase', unknown)
        self.assertEqual(unknown['phase_boundary'], 'unsupported_shape')

    def test_diagnostic_missing_artifact_unsupported_shape_and_field_mismatch_are_distinct(self):
        case = publication.project(self.root)['cases']['ordinary']
        self.assertEqual(case['diagnostics']['browser']['checks_boundary'], 'missing_artifact')
        self.assertEqual(case['diagnostics']['joint_dependency_manifest']['phase_boundary'], 'missing_artifact')
        self.assertEqual(case['correlation_boundaries']['exported'], 'missing_artifact')
        self.write('ordinary/browser-result.json', {'checks': {'UNIT_ONLY_secret': True}})
        self.assertEqual(publication.project(self.root)['cases']['ordinary']['diagnostics']['browser']['checks_boundary'], 'unsupported_shape')
        self.write('ordinary/browser-result.json', {'checks': ['UNIT_ONLY_secret']})
        check = publication.project(self.root)['cases']['ordinary']['diagnostics']['browser']['checks']['items'][0]
        self.assertEqual(check, {'publication_read': 'unsupported_shape'})
        rid = 'history-ordinary-' + 'a' * 16
        self.write('ordinary/telemetry/telemetry-browser-result.json', {'run_id': rid})
        self.write('ordinary/telemetry-delivery.json', {'runId': rid, 'retrieved': []})
        self.write('ordinary/telemetry-final.json', {'exported': []})
        boundaries = publication.project(self.root)['cases']['ordinary']['correlation_boundaries']
        for key in ('browser_runId', 'delivery_run_id', 'exported', 'final_retrieved'):
            self.assertEqual(boundaries[key], 'field_name_mismatch')
        self.assertEqual(boundaries['collector_acceptances'], 'missing_field')
        self.assertEqual(publication.correlation({'kind': 'request', 'request_id': 'a'})['id_boundary'], 'field_name_mismatch')
        for value in (None, [], 'UNIT_ONLY_secret', {'nested': 'UNIT_ONLY_secret'}):
            self.assertEqual(publication.joint_diagnostics({'failure_phase': value})['phase_boundary'],
                             'present' if value is None else 'unsupported_shape')

    def test_actual_uuid_inventory_locations_remain_joinable_without_network_strings(self):
        uid = '12345678-1234-4234-8234-123456789abc'
        request = '22345678-1234-4234-8234-123456789abc'
        stream = '32345678-1234-4234-8234-123456789abc'
        rid = 'history-ordinary-' + 'a' * 16
        secret = 'UNIT_ONLY_NETWORK_HEADER_PATH'
        self.write('ordinary/telemetry/telemetry-browser-result.json', {
            'runId': rid, 'journeys': [{'open_id': uid, 'correlations': [
                {'kind': 'request', 'id': request}, {'kind': 'sse', 'id': stream}],
                'expected': {'server.instance_id': uid, 'server.backend_source': 'b' * 40}}],
            'trials': [{'reports': [{'open_id': uid}],
                'statuses': [{'open_id': uid, 'status': 204, 'headers': secret}],
                'request_facts': [{'kind': 'snapshot', 'open_id': request, 'response_status': 200,
                                   'url': secret}],
                'requestRecords': [{'kind': 'request', 'open_id': request, 'trace_id': 'c' * 32,
                                    'pathname': secret}],
                'requestOwners': [{'journey_id': uid, 'request_id': request, 'path': secret}],
                'streamOwners': [{'journey_id': uid, 'stream_id': stream, 'Cookie': secret}]}]})
        accepted = {'schema_version': 1, 'open_id': uid, 'server_instance_id': uid,
                    'backend_source': 'b' * 40, 'status': 204}
        exported = {'open_id': uid, 'trace_id': 'd' * 32, 'outcome': 'ready'}
        self.write('ordinary/telemetry-delivery.json', {'run_id': rid,
            'collector_acceptances': [accepted], 'exported': [exported], 'service_name': secret})
        self.write('ordinary/telemetry-final.json', {'run_id': rid, 'retrieved': [exported], 'network': secret})
        case = publication.project(self.root)['cases']['ordinary']
        self.assertNotIn(secret, json.dumps(case))
        self.assertEqual(case['collector_acceptances']['items'], [accepted])
        self.assertEqual(case['exported']['items'], [exported])
        self.assertEqual(case['final_export']['items'], [exported])
        self.assertTrue(all(state == 'present' for state in case['correlation_boundaries'].values()))
        self.assertEqual(case['publication_gates']['delivery'], 'typed_inventory_present_not_revalidated')
        trial = case['natural_trials'][0]
        self.assertEqual(trial['reports']['items'][0]['open_id'], accepted['open_id'])
        self.assertEqual(trial['statuses']['items'][0]['open_id'], exported['open_id'])
        self.assertEqual(trial['request_facts']['items'][0]['open_id'], request)
        self.assertEqual(trial['requestRecords']['items'][0]['open_id'], request)
        self.assertEqual(trial['requestOwners']['items'][0], {'journey_id': uid, 'request_id': request})
        self.assertEqual(trial['streamOwners']['items'][0], {'journey_id': uid, 'stream_id': stream})
        # The final receipt can exist before a delivery seal: never relabel it as delivery.
        (self.root / 'ordinary/telemetry-delivery.json').unlink()
        case = publication.project(self.root)['cases']['ordinary']
        self.assertEqual(case['final_export']['items'], [exported])
        self.assertEqual(case['correlation_boundaries']['exported'], 'missing_artifact')

    def test_cleanup_and_check_overflow_remain_bounded(self):
        browser = publication.browser_diagnostics({'checks': list(publication.HISTORY_CHECKS) * 2,
            'cleanup_failures': [{'stage': 'browser_cleanup', 'error': 'UNIT_ONLY_secret'}] * 10,
            'cleanup_overflow': True})
        self.assertEqual(browser['checks']['count'], 24)
        self.assertTrue(browser['checks']['overflow'])
        self.assertEqual(len(browser['checks']['items']), 12)
        self.assertEqual(len(browser['cleanup_failures']['items']), 8)
        self.assertEqual(browser['cleanup_failures']['count'], 10)
        self.assertTrue(browser['cleanup_failures']['overflow'])
        self.assertTrue(browser['cleanup_overflow'])
        self.assertNotIn('UNIT_ONLY_secret', json.dumps(browser))

    def test_missing_malformed_and_symlinked_receipts_are_not_success_or_payload(self):
        value = publication.project(self.root)
        self.assertEqual(value['result'], {'publication_read': 'missing'})
        self.assertIsNone(value['cases']['ordinary']['lifecycle_failure_present'])
        self.assertEqual(value['cases']['ordinary']['publication_gates']['identity'], 'missing_or_invalid_identity_fields')
        self.assertEqual(value['cases']['ordinary']['publication_gates']['delivery'], 'missing_or_invalid_collector_export_inventory')
        self.write('result.json', []).write_text('not JSON UNIT_ONLY_SECRET')
        self.assertEqual(publication.project(self.root)['result'], {'publication_read': 'invalid_json'})
        self.write('identity.json', {'app_source': 'c' * 40})
        (self.root / 'result.json').unlink()
        (self.root / 'result.json').symlink_to(self.root / 'identity.json')
        self.assertEqual(publication.project(self.root)['result'], {'publication_read': 'rejected_symlink'})
        self.assertNotIn('UNIT_ONLY_SECRET', json.dumps(publication.project(self.root)))

    def test_wrong_types_nonfinite_values_and_arbitrary_status_strings_are_excluded(self):
        self.write('result.json', {'status': 'UNIT_ONLY_SECRET', 'runtime_executed': 1,
                                  'joint_telemetry': 'UNIT_ONLY_SECRET', 'elapsed_seconds': float('nan')})
        self.write('ordinary/server-completion.json', {'returncode': True, 'socket_removed': 1})
        self.assertEqual(publication.project(self.root)['result'], {})
        self.assertEqual(publication.project(self.root)['cases']['ordinary']['server_completion'], {})
        self.assertEqual(publication.scalars({'n': 10 ** 1000}, numbers=('n',)), {})
        secret = 'UNIT_ONLY_SECRET'
        rejected = publication.trial({'rawMetrics': {'before': [
            {'name': 'TaskDuration', 'value': float('inf')},
            {'name': secret, 'value': 1}, {'name': 'JSHeapUsedSize', 'value': True}]},
            'metrics': {'TaskDuration': secret, 'JSHeapUsedSize': 10 ** 1000},
            'producer_events': [{'kind': secret, 'ms': secret, 'mark': {'password': secret}}],
            'reports': [{'marks': {'shell': secret, secret: {'cookies': secret}},
                         'total_ms': float('nan'), 'correlations': [{'kind': 'request', 'id': secret}]}]})
        self.assertNotIn(secret, json.dumps(rejected, allow_nan=False))
        self.assertEqual(rejected['metrics'], {})
        self.assertTrue(all(item == {'publication_read': 'invalid_metric'}
                            for item in rejected['rawMetrics']['before']['items']))
        self.assertEqual(rejected['reports']['items'][0]['marks'], {})

    def test_oversize_files_and_trial_overflow_are_explicit_and_bounded(self):
        path = self.write('result.json', {})
        with path.open('wb') as out:
            out.truncate(publication.MAX_BYTES + 1)
        self.assertEqual(publication.project(self.root)['result'], {'publication_read': 'too_large'})
        self.write('ordinary/telemetry/telemetry-browser-result.json', {'trials': [{'posts': 0}] * 50})
        case = publication.project(self.root)['cases']['ordinary']
        self.assertTrue(case['natural_trial_overflow'])
        self.assertEqual(case['natural_trial_count'], 50)
        self.assertEqual(len(case['natural_trials']), 49)


if __name__ == '__main__':
    unittest.main()
