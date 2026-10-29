from pathlib import Path
import subprocess,json,sys,time
stage=sys.argv[1];out=Path('target/ja1-s3-c6a');cargo=str(out/'cargo.sh')
commands=[('build',['build','--workspace']),('clippy',['clippy','--workspace','--all-targets','--','-D','warnings']),('fmt',['fmt','--','--check']),('app',['test','-p','kinewright-app','--no-fail-fast']+(['--features','slow-tests'] if stage=='2' else [])),('media',['test','-p','kinewright-media','--no-fail-fast']+(['--features','slow-tests'] if stage=='2' else [])+['--','--test-threads=1']),('windows',['check','--target','x86_64-pc-windows-msvc','-p','kinewright-app','-p','kinewright-media'])]
records=[]
if len(sys.argv)>2:
 commands=commands[int(sys.argv[2]):]
for name,args in commands:
 path=out/f'gate{stage}-{name}.log';start=time.time()
 with path.open('w') as log:
  log.write('command='+repr([cargo,*args])+'\n');log.flush();run=subprocess.run([cargo,*args],stdout=log,stderr=subprocess.STDOUT)
 record={'gate':name,'command':args,'exit':run.returncode,'seconds':round(time.time()-start,2),'log':str(path)}
 records.append(record);(out/f'gate{stage}-summary.json').write_text(json.dumps(records,indent=2)+'\n');print(stage,name,run.returncode,flush=True)
 if run.returncode!=0:break
