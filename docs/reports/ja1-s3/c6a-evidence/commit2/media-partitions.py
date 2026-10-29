from pathlib import Path
import subprocess,sys,json,re,time
stage=sys.argv[1];out=Path('target/ja1-s3-c6a');cargo=str(out/'cargo.sh');features=['--features','slow-tests'] if stage=='2' else []
base=['test','-p','kinewright-media',*features];records=[]
listing=subprocess.run([cargo,*base,'--lib','--','--list'],stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
(out/f'gate{stage}-media-list.log').write_text(listing.stdout);assert listing.returncode==0
names=[line.removesuffix(': test') for line in listing.stdout.splitlines() if line.endswith(': test')];assert len(names)>1000 and len(names)==len(set(names))
batches=[];pending=[]
for name in names:
 if name.startswith('pf1_s2c_witness::'):
  if pending:batches.append(pending);pending=[]
  batches.append([name])
 else:
  pending.append(name)
  if len(pending)==25:batches.append(pending);pending=[]
if pending:batches.append(pending)
commands=[(f'lib-{i:02}',[*base,'--lib','--','--test-threads=1','--exact',*selected],selected) for i,selected in enumerate(batches)]
for path in sorted(Path('crates/kinewright-media/tests').glob('*.rs')):
 commands.append((path.stem,[*base,'--test',path.stem,'--','--test-threads=1'],None))
commands.append(('doc', [*base,'--doc'],None))
for label,args,selected in commands:
 path=out/f'gate{stage}-media-{label}.log';start=time.time()
 with path.open('w') as log:
  log.write('command='+repr([cargo,*args])+'\n');log.flush();run=subprocess.run([cargo,*args],stdout=log,stderr=subprocess.STDOUT)
 text=path.read_text();summaries=re.findall(r'test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored;',text)
 record={'label':label,'command':args,'selected':selected,'exit':run.returncode,'seconds':round(time.time()-start,2),'raw_summaries':summaries,'log':str(path)}
 records.append(record);(out/f'gate{stage}-media-partitions.json').write_text(json.dumps({'lib_inventory':names,'partitions':records},indent=2)+'\n');print(stage,label,run.returncode,summaries,flush=True)
 assert run.returncode==0,(label,run.returncode,text[-5000:])
 assert summaries,(label,text[-5000:])
 if selected is not None:assert sum(int(p)+int(f)+int(i) for _,p,f,i in summaries)==len(selected)
print('MEDIA_COMPLETE',len(names),'unit entries; all integration targets and doctests',flush=True)
