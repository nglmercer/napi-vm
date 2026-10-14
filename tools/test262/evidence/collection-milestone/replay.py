import json, subprocess, sys
from pathlib import Path
root=Path(__file__).resolve().parents[4]
sys.path.insert(0,str(root/'tools/test262'))
from compare import read_report, compare
before=read_report(root/'tools/test262/evidence/array-mutation-search/full-after.json.gz')
after=read_report(root/'artifacts/test262/milestone-full.json')
transitions=compare(before,after)
(root/'artifacts/test262/milestone-full-transitions.json').write_text(json.dumps(transitions,indent=2)+'\n')
paths=sorted({r['test'] for r in transitions['all_status_transitions']})
(root/'artifacts/test262/milestone-replay-paths.json').write_text(json.dumps(paths,indent=2)+'\n')
for label,worker in [('before','milestone-baseline-worker'),('after','milestone-final-worker')]:
 args=[sys.executable,'tools/test262/run.py','../test262','--engine','artifacts/test262/'+worker,'--revision',after['revision'],'--jobs','4','--timeout','5','--output','artifacts/test262/milestone-replay-'+label+'.json']
 for path in paths:args+=['--path',path]
 with open('/tmp/milestone-replay-'+label+'.log','w') as log:
  result=subprocess.run(args,cwd=root,stdout=log,stderr=subprocess.STDOUT)
  if result.returncode not in (0,1): raise RuntimeError(result.returncode)
  if not (root/('artifacts/test262/milestone-replay-'+label+'.json')).exists(): raise RuntimeError('missing replay report')
replay=compare(read_report(root/'artifacts/test262/milestone-replay-before.json'),read_report(root/'artifacts/test262/milestone-replay-after.json'))
(root/'artifacts/test262/milestone-replay-transitions.json').write_text(json.dumps(replay,indent=2)+'\n')
credited={(r['test'],r['variant']) for r in replay['newly_passing']}
raw={(r['test'],r['variant']) for r in transitions['newly_passing']}
print(json.dumps({'full_new':transitions['new_passes'],'full_lost':transitions['lost_passes'],'replayed_new':len(raw&credited),'unreproduced':sorted(raw-credited),'replay_new':replay['new_passes'],'replay_lost':replay['lost_passes']},indent=2))
