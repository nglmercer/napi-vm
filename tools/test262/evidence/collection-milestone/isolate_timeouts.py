import subprocess,sys,json,time
from pathlib import Path
results=[]
for name,path in [('sort','staging/sm/Array/sort_small.js'),('ownkeys','staging/sm/Proxy/ownkeys-linear.js')]:
 for label,worker in [('before','baseline'),('after','final')]:
  output='artifacts/test262/milestone-isolated-'+name+'-'+label+'.json'
  args=[sys.executable,'tools/test262/run.py','../test262','--engine','artifacts/test262/milestone-'+worker+'-worker','--revision','5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81','--jobs','4','--timeout','5','--path',path,'--output',output]
  start=time.perf_counter()
  with open('/tmp/milestone-isolated-'+name+'-'+label+'.log','w') as log:subprocess.run(args,stdout=log,stderr=subprocess.STDOUT)
  report=json.loads(Path(output).read_text());row={'test':path,'worker':label,'elapsed_seconds':time.perf_counter()-start,'counts':report['counts'],'results':report['results']};results.append(row);print(json.dumps(row),flush=True)
Path('artifacts/test262/milestone-isolated-summary.json').write_text(json.dumps(results,indent=2)+'\n')
