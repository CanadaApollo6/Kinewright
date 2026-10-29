from pathlib import Path
import subprocess,urllib.request,hashlib,json,shlex
root=Path.cwd()/'target/ja1-s3-c6a/speech-tools';root.mkdir(exist_ok=True);records=[]
urls=subprocess.check_output(['pacman','-Sp','--print-format','%l','espeak-ng'],text=True).splitlines()
entries=subprocess.check_output(['bsdtar','-tf','/var/lib/pacman/sync/extra.db'],text=True).splitlines()
for url in urls:
 filename=url.rsplit('/',1)[1];prefix=filename.removesuffix('-x86_64.pkg.tar.zst')
 desc_path=next(name for name in entries if name==prefix+'/desc')
 desc=subprocess.check_output(['bsdtar','-xOf','/var/lib/pacman/sync/extra.db',desc_path],text=True).splitlines()
 expected=desc[desc.index('%SHA256SUM%')+1];archive=root/filename
 with urllib.request.urlopen(url,timeout=60) as response,archive.open('wb') as output:
  while chunk:=response.read(1024*1024):output.write(chunk)
 actual=hashlib.sha256(archive.read_bytes()).hexdigest();assert actual==expected,(filename,actual,expected)
 subprocess.run(['bsdtar','-xf',str(archive),'-C',str(root)],check=True)
 records.append({'url':url,'sha256':actual,'metadata':'/var/lib/pacman/sync/extra.db:'+desc_path})
(root/'bin').mkdir(exist_ok=True)
wrapper=root/'bin/espeak-ng';wrapper.write_text('#!/usr/bin/env bash\nset -eu\nROOT='+shlex.quote(str(root))+'\nexport LD_LIBRARY_PATH="$ROOT/usr/lib:${LD_LIBRARY_PATH:-}"\nexec "$ROOT/usr/bin/espeak-ng" --path="$ROOT/usr/share" "$@"\n');wrapper.chmod(0o755)
(root/'packages.json').write_text(json.dumps(records,indent=2)+'\n')
subprocess.run([str(wrapper),'--version'],check=True)
