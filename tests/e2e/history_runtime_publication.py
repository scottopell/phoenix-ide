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


def correlation(value):
    value = object_value(value)
    out = scalars(value, enums={'kind': ('request', 'sse', 'trace')})
    out.update(identifiers(value, traces=('id',)) if value.get('kind') == 'trace'
               else identifiers(value, uuids=('id',)))
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
        case = {
            'authentication': scalars(read('authentication.json'),
                                      booleans=('auth_required', 'parent_authenticated')),
            'server_completion': scalars(completion,
                booleans=('socket_removed', 'owned_home_removed'), numbers=('returncode',),
                enums={'status': ('success', 'failed', 'timeout', 'cleanup_failed', 'nonzero_exit',
                                  'signal_exit', 'force_cleanup_required', 'stop_request_failed')}),
            'browser': scalars(browser, enums={'status': ('failed', 'passed_assertions')}),
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
            'telemetry_evidence': scalars(telemetry, enums={'evidence_class': ('stock_natural',),
                'status': ('failed', 'passed_assertions')}),
            'run_correlation': run_correlation(telemetry),
            'delivery_correlation': run_correlation(delivery),
            'collector_acceptances': bounded_records(delivery, 'collector_acceptances', 64, acceptance),
            'exported': bounded_records(delivery, 'exported', 64, acceptance),
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
