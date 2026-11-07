"""A partial or failed suite must never advertise all-group completion."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

SPEC=importlib.util.spec_from_file_location('suite',Path(__file__).resolve().parents[1]/'suite.py')
SUITE=importlib.util.module_from_spec(SPEC);SPEC.loader.exec_module(SUITE)

class SuiteTests(unittest.TestCase):
    def run_suite(self,groups,failed_validation=False,empty_report=False):
        temp=tempfile.TemporaryDirectory();self.addCleanup(temp.cleanup)
        output=Path(temp.name)/'capture'
        def child(args,**kwargs):
            if failed_validation and args[1:3]==['bench/http.py','compare']:
                return subprocess.CompletedProcess(args,7)
            if '--output' in args:
                target=Path(args[args.index('--output')+1])
                if args[1]=='bench/performance.py':
                    target.mkdir();target=target/'baseline.json'
                elif 'build' in args:
                    target.mkdir();return subprocess.CompletedProcess(args,0)
                target.write_text(json.dumps({'schema':1,'results':[] if empty_report else [{'name':'synthetic child report'}]}))
            return subprocess.CompletedProcess(args,0)
        argv=['suite.py','--output',str(output),'--groups',groups,'--samples','3','--iterations','1']
        with patch.object(sys,'argv',argv),patch.object(SUITE.subprocess,'run',side_effect=child),contextlib.redirect_stdout(io.StringIO()):
            if failed_validation or empty_report:
                with self.assertRaises((RuntimeError,ValueError)):SUITE.main()
            else:SUITE.main()
        return json.loads((output/'suite.json').read_text())

    def test_complete_and_partial_selections_are_distinct(self):
        all_groups=self.run_suite('engines,memory,http,host,platforms,async,international')
        self.assertTrue(all_groups['all_groups_complete'])
        self.assertEqual(set(r['group'] for r in all_groups['reports']),set(SUITE.GROUPS))
        partial=self.run_suite('engines')
        self.assertEqual(partial['status'],'complete')
        self.assertFalse(partial['all_groups_complete'])
        self.assertEqual(partial['unselected_groups'],['memory','http','host','platforms','async','international'])
        legacy=self.run_suite('engines,memory,http,host')
        self.assertFalse(legacy['all_groups_complete'])
        self.assertEqual(legacy['unselected_groups'],['platforms','async','international'])
        previous=self.run_suite('engines,memory,http,host,platforms')
        self.assertFalse(previous['all_groups_complete'])
        self.assertEqual(previous['unselected_groups'],['async','international'])
        six=self.run_suite('engines,memory,http,host,platforms,async')
        self.assertFalse(six['all_groups_complete'])
        self.assertEqual(six['unselected_groups'],['international'])

    def test_failed_or_empty_child_does_not_complete_suite(self):
        for settings in [dict(failed_validation=True),dict(empty_report=True)]:
            with self.subTest(settings=settings):
                report=self.run_suite('engines,http',**settings)
                self.assertEqual(report['status'],'failed')
                self.assertFalse(report['all_groups_complete'])
                self.assertIn('error',report)

if __name__=='__main__':unittest.main()
