#!/usr/bin/env python3
"""Project stock synthetic runtime distributions; never copy raw evidence."""
import argparse
import json
import math
from pathlib import Path
import re


MAX_BYTES = 8 * 1024 * 1024


def read_receipt(root, relative):
    path = root / relative
    if any(part.is_symlink() for part in (path, *path.parents)):
        return {'publication_read': 'rejected_symlink'}
    if not path.is_file():
        return {'publication_read': 'missing'}
    if path.stat().st_size > MAX_BYTES:
        return {'publication_read': 'too_large'}
    try:
        value = json.loads(path.read_text())
    except (ValueError, UnicodeError):
        return {'publication_read': 'invalid_json'}
    return value if isinstance(value, dict) else {'publication_read': 'invalid_shape'}


def scalars(value, *, booleans=(), numbers=(), enums=None):
    value = object_value(value)
    out = {}
    for key in booleans:
        if type(value.get(key)) is bool:
            out[key] = value[key]
    for key in numbers:
        item = value.get(key)
        if type(item) in (int, float) and abs(item) <= 1e12 and math.isfinite(item):
            out[key] = item
    for key, choices in (enums or {}).items():
        if type(value.get(key)) is str and value[key] in choices:
            out[key] = value[key]
    if 'publication_read' in value:
        # Never trust a producer-supplied arbitrary string, even on failure.
        state = value['publication_read']
        if state in ('missing', 'invalid_json', 'invalid_shape', 'too_large', 'rejected_symlink'):
            out['publication_read'] = state
    return out


def inventory(value, key):
    items = object_value(value).get(key)
    return len(items) if isinstance(items, list) else None


# Performance.getMetrics names, not producer-controlled names or arbitrary strings.
CDP_METRICS = ('Timestamp', 'AudioHandlers', 'AudioWorkletProcessors', 'Documents',
    'Frames', 'JSEventListeners', 'LayoutObjects', 'MediaKeySessions', 'MediaKeys',
    'Nodes', 'Resources', 'ContextLifecycleStateObservers', 'V8PerContextDatas',
    'WorkerGlobalScopes', 'UACSSResources', 'RTCPeerConnections', 'ResourceFetchers',
    'AdSubframes', 'DetachedScriptStates', 'ArrayBufferContents', 'LayoutCount',
    'RecalcStyleCount', 'LayoutDuration', 'RecalcStyleDuration', 'DevToolsCommandDuration',
    'ScriptDuration', 'V8CompileDuration', 'TaskDuration', 'TaskOtherDuration',
    'ThreadTime', 'ProcessTime', 'JSHeapUsedSize', 'JSHeapTotalSize',
    'FirstMeaningfulPaint', 'DomContentLoaded', 'NavigationStart')
MARKS = ('shell', 'snapshot_received', 'hydration_ready', 'meaningful_transcript',
         'chapter_data_ready', 'breadcrumb_presented', 'breadcrumb_enabled', 'anchor_settled')
OUTCOMES = ('ready', 'failed', 'canceled', 'incomplete')
ROUTES = ('ordinary', 'global')
REPORT_NUMBERS = ('total_ms', 'background_ms', 'background_interval_count',
    'observer_inspection_count', 'breadcrumb_ready_minus_meaningful_ms',
    'dropped_correlations', 'sample_rate', 'admitted_count', 'sampled_count',
    'transport_dropped_count', 'transport_failed_count', 'reconnect_count')
REPORT_BOOLS = ('standalone', 'frontend_source_dirty', 'visible', 'timeline_saturated',
                'background_open', 'observer_scan_limited', 'counters_saturated')


# qualify-history-opening-realserver.mjs checked() records completed assertions only.
HISTORY_CHECKS = (
    'manual equal-text accepted opening remains separate',
    'Global earlier-history acquisition failed while Pending before release',
    'Global declared postfault physical historical baseline while Pending',
    'committed Pending: browser/HTTP/new SSE init available, opening deferred and composer readonly',
    'acceptance retains physical anchor',
    'actual successor chat ACK receipt',
    'accepted SSE unblocks exact successor composer despite actual aggregate GET failure',
    'reconnect retains physical anchor',
    'same-page transport SSE drop/reconnect preserves accepted full-edge identity',
    'ordinary chapter ownedGuard acquisition uses restored real aggregate transport',
    'Global cache-backed chapter selection with observed browser request slice',
    'accepted manual opening renders once with exact persisted identity after real chapter click',
)
JOINT_PHASES = ('runtime_import', 'export_admission', 'context_acquisition',
    'session_authentication', 'page_acquisition', 'module_checks', 'result_validation')
BROWSER_CLEANUP_STAGES = ('authentication_response', 'recorder_drain', 'history_gate',
    'joint_browser_cleanup', 'server_stop', 'browser_cleanup', 'recorder_final')
LIFECYCLE_STAGES = ('receiver', 'server', 'preview', 'browser', 'server_body', 'case',
    'normal_completion', 'normal_completion_receipt', 'server_control_stop',
    'failure_receipt', 'failure_drain')


def object_value(value):
    return value if isinstance(value, dict) else {}


def identifiers(value, uuids=(), hashes=(), traces=()):
    value = object_value(value)
    out = {}
    for keys, pattern in ((uuids, r'[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}'),
                          (hashes, r'(?:[a-f0-9]{40}|[a-f0-9]{64})'),
                          (traces, r'[a-f0-9]{32}')):
        for key in keys:
            item = value.get(key)
            if type(item) is str and re.fullmatch(pattern, item) and set(item) != {'0'}:
                out[key] = item
    return out


def numeric_fields(value, keys):
    value = object_value(value)
    out = scalars(value, numbers=keys)
    for key in keys:
        if key in value and value[key] is None:
            out[key] = None
    return out


def bounded_records(value, key, limit, projector):
    items = object_value(value).get(key)
    if not isinstance(items, list):
        return {'publication_read': 'missing_or_invalid_list'}
    return {'count': len(items), 'overflow': len(items) > limit,
            'items': [projector(item) for item in items[:limit]]}


def field_boundary(value, key, valid, *, aliases=()):
    value = object_value(value)
    read = scalars(value).get('publication_read')
    if read:
        return 'missing_artifact' if read == 'missing' else read
    if key not in value:
        return 'field_name_mismatch' if any(alias in value for alias in aliases) else 'missing_field'
    return 'present' if valid else 'unsupported_shape'


def failure_presence(value, keys):
    value = object_value(value)
    return {key + '_present': key in value and value[key] is not None for key in keys}


def failure_record(value, stages, text_key):
    value = object_value(value)
    out = scalars(value, booleans=('truncated', 'message_truncated'), enums={'stage': stages})
    out.update(failure_presence(value, (text_key,)))
    out['stage_boundary'] = field_boundary(value, 'stage', 'stage' in out)
    return out


def browser_diagnostics(value):
    value = object_value(value)
    def check(item):
        return ({'name': item, 'passed': True} if type(item) is str and item in HISTORY_CHECKS
                else {'publication_read': 'unsupported_shape'})
    out = scalars(value, booleans=('cleanup_overflow',))
    out.update(failure_presence(value, ('error',)))
    out['checks'] = bounded_records(value, 'checks', len(HISTORY_CHECKS), check)
    out['checks_boundary'] = field_boundary(value, 'checks', isinstance(value.get('checks'), list))
    out['cleanup_failures'] = bounded_records(value, 'cleanup_failures', 8,
        lambda item: failure_record(item, BROWSER_CLEANUP_STAGES, 'error'))
    out['cleanup_boundary'] = field_boundary(value, 'cleanup_failures', isinstance(value.get('cleanup_failures'), list))
    for key in ('application_page_errors', 'cdp_capture_errors'):
        out[key + '_count'] = inventory(value, key)
    return out


def joint_diagnostics(value):
    value = object_value(value)
    out = scalars(value, booleans=('context_acquired',), enums={'failure_phase': JOINT_PHASES})
    if 'failure_phase' in value and value['failure_phase'] is None:
        out['failure_phase'] = None
    out['phase_boundary'] = field_boundary(value, 'failure_phase', 'failure_phase' in out)
    out.update(failure_presence(value, ('primary_failure', 'authentication_response_cleanup_failure')))
    out['cleanup_failure_count'] = inventory(value, 'cleanup_failures')
    out['cleanup_boundary'] = field_boundary(value, 'cleanup_failures', isinstance(value.get('cleanup_failures'), list))
    return out


def lifecycle_diagnostics(value):
    value = object_value(value)
    out = scalars(value, booleans=('cleanup_overflow',))
    out['primary'] = (None if value.get('primary') is None else
                      failure_record(value['primary'], LIFECYCLE_STAGES, 'message'))
    out['primary_boundary'] = field_boundary(value, 'primary', value.get('primary') is None
                                            or isinstance(value.get('primary'), dict))
    out['cleanup'] = bounded_records(value, 'cleanup', 8,
        lambda item: failure_record(item, LIFECYCLE_STAGES, 'message'))
    out['cleanup_boundary'] = field_boundary(value, 'cleanup', isinstance(value.get('cleanup'), list))
    return out


def correlation(value):
    value = object_value(value)
    out = scalars(value, enums={'kind': ('request', 'sse', 'trace')})
    out.update(identifiers(value, traces=('id',)) if value.get('kind') == 'trace'
               else identifiers(value, uuids=('id',)))
    out['id_boundary'] = field_boundary(value, 'id', 'id' in out,
                                        aliases=('request_id', 'stream_id', 'trace_id'))
    return out


def report(value):
    value = object_value(value)
    out = scalars(value, booleans=REPORT_BOOLS, numbers=REPORT_NUMBERS,
        enums={'route': ROUTES, 'outcome': OUTCOMES,
               'trigger': ('navigation', 'reload', 'back_forward', 'pwa_resume'),
               'presentation_evidence': ('commit_frame_estimate',),
               'anchor_result': ('applied', 'tail_pinned', 'empty', 'target_missing', 'superseded')})
    out.update(identifiers(value, uuids=('open_id', 'reporter_id', 'product_conversation_id', 'segment_id'),
                           hashes=('frontend_source',)))
    out['marks'] = numeric_fields(value.get('marks'), MARKS)
    out['correlations'] = bounded_records(value, 'correlations', 16, correlation)
    return out


def producer_event(value):
    value = object_value(value)
    out = scalars(value, booleans=('sampled',), numbers=('version', 'ms', 'status'),
        enums={'kind': ('admission', 'mark', 'request', 'sse', 'anchor', 'terminal', 'submission'),
               'route': ROUTES, 'mark': MARKS, 'outcome': OUTCOMES,
               'result': ('applied', 'tail_pinned', 'empty', 'target_missing', 'superseded'),
               'state': ('attempted', 'timeout_abort', 'busy_drop', 'response', 'failed'),
               'observed_phase': ('open', 'closing')})
    out.update(identifiers(value, uuids=('open_id', 'request_id', 'stream_id')))
    for key in ('open_id', 'status'):
        if key in value and value[key] is None:
            out[key] = None
    return out


def run_correlation(value):
    out = {}
    for key in ('runId', 'run_id'):
        item = object_value(value).get(key)
        if type(item) is str and re.fullmatch(r'history-(?:ordinary|global)-[a-f0-9]{16}', item):
            out[key] = item
    return out


def metric(value):
    out = scalars(object_value(value), numbers=('value',), enums={'name': CDP_METRICS})
    return out if set(out) == {'name', 'value'} else {'publication_read': 'invalid_metric'}


def request_fact(value):
    return (scalars(value, booleans=('request_failed', 'has_before', 'finished', 'failed'),
                    numbers=('ordinal', 'response_status', 'status'),
                    enums={'kind': ('report', 'snapshot', 'global_history', 'request', 'sse'),
                           'observed_phase': ('open', 'closing')})
            | identifiers(value, uuids=('open_id',), traces=('trace_id',)))


def trial(value):
    value = object_value(value)
    out = scalars(value, booleans=('sampled', 'warmup', 'context_closed', 'metric_reset'),
        numbers=('index', 'attempt', 'posts', 'payloadBytes', 'correlatedRequests',
                 'navigation_to_visible_readiness_ms'),
        enums={'route': ROUTES,
               'kind': ('pending_background', 'spa_canceled', 'bfcache_resume'),
               'status': ('observed_history_inventory', 'started', 'unsampled', 'sampled_already_ready',
                          'inconclusive_explicit_unsampled', 'provisional_incomplete_collector_export_pending',
                          'observed_spa_canceled', 'actual_hidden_response_then_visible_readiness',
                          'unsupported_no_actual_hidden_visibility', 'unsupported_no_actual_bfcache_restore',
                          'actual_bfcache_restore_observed', 'actual_bfcache_restore_no_resume_report_sampling_unknown',
                          'inconclusive_readiness_preceded_finite_request_sampling_unknown',
                          'observed_bfcache_fresh_ready_report'),
               'readiness_clock': ('driver_monotonic_includes_automation_latency_not_paint',),
               'metric_method': ('navigation-scoped post-readiness raw counters; no aboutblank subtraction',),
               'close_time_producer_delivery': ('not_proven',)})
    out.update(identifiers(value, uuids=('admitted_open_id', 'requestId')))
    out.update({key + '_present': key in value for key in ('failure', 'cleanup_failure')})
    out['capture_error_count'] = inventory(value, 'capture_errors')
    out['metrics'] = scalars(object_value(value.get('metrics')), numbers=CDP_METRICS)
    raw = object_value(value.get('rawMetrics'))
    out['rawMetrics'] = {phase: bounded_records(raw, phase, 64, metric) for phase in ('before', 'after')}
    out['witness'] = numeric_fields(object_value(value.get('witness')), ('foregroundAt',))
    out['observer'] = scalars(object_value(value.get('observer')), booleans=('scan_limited',),
        numbers=('inspection_count', 'background_ms', 'background_interval_count', 'sample_rate',
                 'admitted_count', 'sampled_count', 'transport_dropped_count', 'transport_failed_count'))
    out['reports'] = bounded_records(value, 'reports', 16, report)
    out['producer_events'] = bounded_records(value, 'producer_events', 64, producer_event)
    # qualify-view-telemetry and the appended history inventory use distinct field names.
    out['statuses'] = bounded_records(value, 'statuses', 16,
        lambda item: (scalars(item, numbers=('status',)) | identifiers(item, uuids=('open_id',)))
        if isinstance(item, dict) else scalars({'status': item}, numbers=('status',)))
    out['request_facts'] = bounded_records(value, 'request_facts', 32, request_fact)
    out['requestRecords'] = bounded_records(value, 'requestRecords', 128, request_fact)
    for key, id_key in (('requestOwners', 'request_id'), ('streamOwners', 'stream_id')):
        out[key] = bounded_records(value, key, 512,
            lambda item, id_key=id_key: identifiers(item, uuids=('journey_id', id_key)))
    out.update(failure_presence(value, ('ownershipFailure', 'callback_failure', 'persistence_failure')))
    out['response_statuses'] = bounded_records(value, 'response_statuses', 16,
                                               lambda item: scalars({'status': item}, numbers=('status',)))
    return out


def journey(value):
    value = object_value(value)
    out = identifiers(value, uuids=('open_id',))
    expected = object_value(value.get('expected'))
    out['expected'] = numeric_fields(expected, tuple('browser.' + mark + '_ms' for mark in MARKS)
        + ('browser.total_ms', 'browser.breadcrumb_ready_minus_meaningful_ms'))
    out['expected'].update(identifiers(expected, uuids=('server.instance_id', 'product_conversation.id'),
        hashes=('server.backend_source', 'browser.frontend_source')))
    out['expected'].update(scalars(expected, enums={'browser.route': ROUTES, 'browser.outcome': OUTCOMES,
        'browser.trigger': ('navigation', 'reload', 'back_forward', 'pwa_resume'),
        'browser.presentation_evidence': ('commit_frame_estimate',)}))
    out['correlations'] = bounded_records(value, 'correlations', 16, correlation)
    return out


def acceptance(value):
    value = object_value(value)
    return (scalars(value, numbers=('schema_version', 'status'), enums={'outcome': OUTCOMES})
            | identifiers(value, uuids=('open_id', 'server_instance_id'),
                          hashes=('backend_source',), traces=('trace_id',)))


def project(root):
    result = read_receipt(root, 'result.json')
    identity = read_receipt(root, 'identity.json')
    hashes = {key: value for key, value in identity.items()
              if key in ('app_source', 'app_tree', 'app_base', 'harness_source',
                         'executable_sha256', 'receipt_sha256', 'cargo_lock_sha256', 'ui_lock_sha256')
              and type(value) is str and re.fullmatch('[a-f0-9]{40}|[a-f0-9]{64}', value)}
    publication = {
        'schema_version': 1,
        'scope': 'stock-only typed raw distributions, not raw payloads or shipping/overhead clearance',
        'full_acceptance': False, 'overhead_clearance': False,
        'failure_presence': failure_presence(result, ('error',)),
        'aggregation_error_count': inventory(result, 'aggregation_errors'),
        'result': scalars(result, booleans=('runtime_executed', 'joint_telemetry'),
                          numbers=('elapsed_seconds',), enums={'status': ('failed', 'passed_assertions'),
                          'telemetry_evidence_class': ('stock_natural', 'controlled_functional')}),
        'ui_artifacts': {mode: identifiers(object_value(identity.get('ui_artifacts')).get(mode),
                                            hashes=('manifest_sha256',))
                         for mode in ('stock_natural', 'controlled_functional')},
        'identity': hashes | scalars(identity, enums={'evidence_class': ('instrumented_cfg_test_library_not_shipping_runtime',)}), 'cases': {},
    }
    for kind in ('ordinary', 'global'):
        read = lambda name: read_receipt(root, kind + '/' + name)
        browser = read('browser-result.json')
        telemetry = read('telemetry/telemetry-browser-result.json')
        completion = read('server-completion.json')
        failures = read('lifecycle-failures.json')
        delivery = read('telemetry-delivery.json')
        final = read('telemetry-final.json')
        joint = read('joint-telemetry-dependency-manifest.json')
        case = {
            'authentication': scalars(read('authentication.json'),
                                      booleans=('auth_required', 'parent_authenticated')),
            'server_completion': scalars(completion,
                booleans=('socket_removed', 'owned_home_removed'), numbers=('returncode',),
                enums={'status': ('success', 'failed', 'timeout', 'cleanup_failed', 'nonzero_exit',
                                  'signal_exit', 'force_cleanup_required', 'stop_request_failed')}),
            'browser': scalars(browser, enums={'status': ('failed', 'passed_assertions')}),
            'diagnostics': {'browser': browser_diagnostics(browser),
                            'joint_dependency_manifest': joint_diagnostics(joint),
                            'lifecycle_failures': lifecycle_diagnostics(failures)},
            'browser_check_count': inventory(browser, 'checks'),
            'lifecycle_failure_present': failures.get('primary') is not None if 'primary' in failures else None,
            'cleanup_failure_count': inventory(failures, 'cleanup'),
            'drain': scalars(read('telemetry-drain.json'), booleans=('app_stopped', 'receiver_stopped')),
            'delivery': scalars(delivery, booleans=('normal_drain',),
                                enums={'evidence_class': ('stock_natural', 'controlled_functional')}),
            'collector_acceptance_count': inventory(delivery, 'collector_acceptances'),
            'exported_count': inventory(delivery, 'exported'),
            'identity': identifiers(telemetry.get('identity'), uuids=('serverInstanceId',),
                                    hashes=('appSource', 'frontendSource')),
            'telemetry_failure_present': 'failure' in telemetry,
            'telemetry_cleanup_failure_present': 'cleanup_failure' in telemetry,
            'telemetry_context_cleanup_failure_present': 'context_cleanup_failure' in telemetry,
            'telemetry_evidence': scalars(telemetry, enums={'evidence_class': ('stock_natural',),
                'status': ('failed', 'passed_assertions')}),
            'run_correlation': run_correlation(telemetry),
            'delivery_correlation': run_correlation(delivery),
            'collector_acceptances': bounded_records(delivery, 'collector_acceptances', 64, acceptance),
            'exported': bounded_records(delivery, 'exported', 64, acceptance),
            'final_export': bounded_records(final, 'retrieved', 64, acceptance),
            'export_correlation': run_correlation(final),
            'correlation_boundaries': {
                'browser_runId': field_boundary(telemetry, 'runId', 'runId' in run_correlation(telemetry), aliases=('run_id',)),
                'delivery_run_id': field_boundary(delivery, 'run_id', 'run_id' in run_correlation(delivery), aliases=('runId',)),
                'final_run_id': field_boundary(final, 'run_id', 'run_id' in run_correlation(final), aliases=('runId',)),
                'collector_acceptances': field_boundary(delivery, 'collector_acceptances', isinstance(delivery.get('collector_acceptances'), list)),
                'exported': field_boundary(delivery, 'exported', isinstance(delivery.get('exported'), list), aliases=('retrieved',)),
                'final_retrieved': field_boundary(final, 'retrieved', isinstance(final.get('retrieved'), list), aliases=('exported',)),
            },
            'journeys': bounded_records(telemetry, 'journeys', 64, journey),
            'stages': {stage: bounded_records(object_value(telemetry.get(stage)), 'trials', 64, trial)
                       for stage in ('terminal', 'lifecycle')},
            'publication_gates': {
                'cdp_deltas': 'not_emitted_by_pinned_driver_no_derived_subtraction',
                'long_tasks': 'not_emitted_by_pinned_driver',
                'heap': 'raw_CDP_JSHeapUsedSize_not_post_GC_retained_heap',
                'wall_time': 'driver_navigation_to_visible_readiness_not_paint_or_total_wall_time',
                'non_allowlisted_fields': 'omitted_including_free_form_strings_and_network_paths',
                'invalid_fields': 'missing_invalid_or_out_of_bounds_fields_omitted_not_success',
                'telemetry_evidence_class': 'not_emitted_by_pinned_browser_driver_see_result_and_build_identity',
            },
            'natural_trials': [],
        }
        trials = telemetry.get('trials', [])
        case['natural_trial_count'] = inventory(telemetry, 'trials')
        case['natural_trial_overflow'] = isinstance(trials, list) and len(trials) > 49
        if isinstance(trials, list):
            case['natural_trials'] = [trial(item) for item in trials[:49]]
        case['publication_gates']['identity'] = (
            'present' if set(case['identity']) == {'serverInstanceId', 'appSource', 'frontendSource'}
            else 'missing_or_invalid_identity_fields')
        case['publication_gates']['delivery'] = (
            'typed_inventory_present_not_revalidated' if (case['collector_acceptances'].get('items')
                and case['exported'].get('items')
                and all(set(item) >= {'open_id', 'server_instance_id', 'backend_source', 'status', 'schema_version'}
                        for item in case['collector_acceptances']['items'])
                and all(set(item) >= {'open_id', 'trace_id', 'outcome'} for item in case['exported']['items']))
            else 'missing_or_invalid_collector_export_inventory')
        case['publication_gates']['run_correlation'] = (
            'typed_run_ids_present' if case['run_correlation'] and case['delivery_correlation']
            else 'missing_or_invalid_run_ids')
        publication['cases'][kind] = case
    return publication


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--input', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    with args.output.open('x') as output:
        json.dump(project(args.input), output, indent=2, allow_nan=False)
        output.write('\n')


if __name__ == '__main__':
    main()
