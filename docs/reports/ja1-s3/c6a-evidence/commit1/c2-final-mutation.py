import subprocess,hashlib,json
from pathlib import Path
root=Path.cwd();out=root/'target/ja1-s3-c6a';app=root/'crates/kinewright-app/src'
cases=[
 ('second-owner-save', app/'studio_feedback.rs','''        if let Keeping::Failed { why, .. } = &self.keeping {
            return Outcome::Refused(why.clone());
        }
''','', 's3_fix3_one_owner_per_recovery_file','C6A_SECOND_OWNER_NO_ADMISSION',1),
 ('second-owner-submit', app/'studio_feedback.rs','''        if let Keeping::Failed { why, .. } = &self.keeping {
            return Outcome::Refused(why.clone());
        }
''','', 's3_fix3_one_owner_per_recovery_file','C6A_SECOND_OWNER_NO_ISSUER',2),
 ('frozen-manual-trim', app/'app.rs','''                    self.projects[project_index]
                        .studio
                        .update_manual_document(Arc::clone(&doc));
''','', 's3_current_edit_refreshes_trim_while_frozen','C6A_MANUAL_TRIM_REFRESHED',1),
]
results=[]
for name,path,old,new,selector,marker,occurrence in cases[-1:]:
 original=path.read_bytes();source=original.decode();assert source.count(old)>=occurrence,(name,source.count(old))
 at=-1
 for _ in range(occurrence):at=source.index(old,at+1)
 mutant=source[:at]+new+source[at+len(old):]
 record={'name':name,'source':str(path.relative_to(root)),'selector':selector,'assertion':marker}
 try:
  for phase,data in [('red',mutant.encode()),('green',original)]:
   path.write_bytes(data);sha=hashlib.sha256(data).hexdigest();log=out/f'{name}-{phase}.log'
   with log.open('w') as f:
    f.write(f'guarded_source={record["source"]} sha256={sha}\n');f.flush()
    run=subprocess.run([str(out/'cargo.sh'),'test','-p','kinewright-app',selector,'--','--nocapture'],stdout=f,stderr=subprocess.STDOUT)
   text=log.read_text();record[phase]={'exit':run.returncode,'sha256':sha,'log':str(log.relative_to(root))}
   if phase=='red':assert run.returncode==101 and marker in text,(name,run.returncode,text[-2500:])
   else:assert run.returncode==0,(name,text[-2500:])
 finally:path.write_bytes(original)
 results.append(record);(out/'c2-final-mutation.json').write_text(json.dumps(results,indent=2)+'\n')
 print(name,'red 101 / green 0',flush=True)
