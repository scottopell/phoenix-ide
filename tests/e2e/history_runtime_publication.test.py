#!/usr/bin/env python3
"""Synthetic filesystem tests only: no SQL, HTTP, browser, or app execution."""
import json
from pathlib import Path
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
