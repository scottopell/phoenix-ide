"""Credential-free checks on model-visible synthetic evidence and hidden rubrics."""
import json
import unittest
from pathlib import Path

ROOT = Path(__file__).parent

class FixtureTests(unittest.TestCase):
    def test_six_cases_hidden_outcomes_and_provenance(self):
        fixture = json.loads((ROOT / 'fixtures.json').read_text())
        rubrics = json.loads((ROOT / 'expected.json').read_text())
        ids = [s['id'] for s in fixture['scenarios']]
        self.assertEqual(ids, ['idle_owner', 'approved_routine_failure', 'genuine_gate_other_lane', 'quiet_subscription', 'executive_checkin', 'continuation_pause'])
        self.assertEqual(set(ids), {key for key in rubrics if not key.startswith('_')})
        for case in fixture['scenarios']:
            self.assertNotIn('expected', case)
            self.assertTrue(case['rows'] and case['context'] and case['conversations'])
            for messages in case['conversations'].values():
                self.assertTrue(all(m['origin'] in ('user_api', 'system_generated') for m in messages))
        self.assertEqual(fixture['variants']['B']['button'], fixture['variants']['C']['button'])
        self.assertIn('Do not send messages, change anything', fixture['variants']['A']['button'])
        self.assertEqual(fixture['limits']['max_provider_calls_per_trial'], 10)
        self.assertEqual(fixture['limits']['max_output_tokens'], 3072)

if __name__ == '__main__':
    unittest.main()
