#!/usr/bin/env python3
"""Synthetic archive provenance witnesses; raw extractor and complete unpack observations."""
import os,tempfile,pathlib,subprocess,tarfile,zipfile,gzip,json
import ctypes,errno
lib=ctypes.CDLL(None,use_errno=True)
lib.getxattr.restype=ctypes.c_ssize_t
lib.getxattr.argtypes=[ctypes.c_char_p,ctypes.c_char_p,ctypes.c_void_p,ctypes.c_size_t,ctypes.c_uint32,ctypes.c_int]
lib.setxattr.argtypes=[ctypes.c_char_p,ctypes.c_char_p,ctypes.c_void_p,ctypes.c_size_t,ctypes.c_uint32,ctypes.c_int]
def set_attr(p,n,v):
 if lib.setxattr(os.fsencode(p),n.encode(),v,len(v),0,1)!=0:raise OSError(ctypes.get_errno(),'setxattr failed')
def get_attr(p,n):
 size=lib.getxattr(os.fsencode(p),n.encode(),None,0,0,1)
 if size<0:raise OSError(ctypes.get_errno(),'getxattr failed')
 b=ctypes.create_string_buffer(size)
 if lib.getxattr(os.fsencode(p),n.encode(),b,size,0,1)<0:raise OSError(ctypes.get_errno(),'getxattr failed')
 return b.raw
mark=b'0083;00000000;etudes-synthetic;00000000-0000-0000-0000-000000000000'

import hashlib,sys,io
FORMATS=[('.zip',None),('.jar',None),('.tar','-xf'),('.tar.gz','-xzf'),('.tgz','-xzf'),('.tar.bz2','-xjf'),('.tbz','-xjf'),('.tar.xz','-xJf'),('.txz','-xJf'),('.gz',None)]
def make_archive(path,suffix,source):
 if suffix in ('.zip','.jar'):
  with zipfile.ZipFile(path,'w') as z:z.write(source,'wrapper/nested/run.sh')
 elif suffix=='.gz':path.write_bytes(gzip.compress(source.read_bytes()))
 else:
  mode='w'+({'.tar':'','.tar.gz':':gz','.tgz':':gz','.tar.bz2':':bz2','.tbz':':bz2','.tar.xz':':xz','.txz':':xz'}[suffix])
  with tarfile.open(path,mode) as t:t.add(source,'wrapper/nested/run.sh')
def read_optional(path):
 try:return get_attr(path,'com.apple.quarantine')
 except OSError as error:
  if error.errno==93:return None
  raise

def raw(root):
 source=root/'source';source.write_bytes(b'#!/bin/sh\nexit 0\n');source.chmod(0o755)
 for index,(suffix,flag) in enumerate(FORMATS):
  archive=root/f'raw-{index}{suffix}';make_archive(archive,suffix,source)
  set_attr(archive,'com.apple.quarantine',mark);assert get_attr(archive,'com.apple.quarantine')==mark
  out=root/f'raw-{index}';out.mkdir()
  if suffix in ('.zip','.jar'):subprocess.run(['/usr/bin/unzip','-o','-q',str(archive),'-d',str(out)],check=True)
  elif suffix=='.gz':
   with open(out/'run.sh','wb') as target:subprocess.run(['/usr/bin/gunzip','-c',str(archive)],stdout=target,check=True)
  else:subprocess.run(['/usr/bin/tar',flag,str(archive),'-C',str(out)],check=True)
  result=read_optional(out/('run.sh' if suffix=='.gz' else 'wrapper/nested/run.sh'))
  state='exact' if result==mark else ('absent' if result is None else 'changed')
  print(f'raw {suffix}: '+state,flush=True)

def unpack(root,binary,destination_root):
 source=root/'source';source.write_bytes(b'#!/bin/sh\nexit 0\n');source.chmod(0o755)
 for marked in (False,True):
  for index,(suffix,flag) in enumerate(FORMATS):
   archive=root/f'archive-{marked}-{index}{suffix}';make_archive(archive,suffix,source)
   if marked:set_attr(archive,'com.apple.quarantine',mark)
   before=hashlib.sha256(archive.read_bytes()).hexdigest();before_mark=read_optional(archive)
   dest=destination_root/f'out-{marked}-{index}'
   result=subprocess.run([str(binary),str(archive),'--into',str(dest),'--json'],capture_output=True,text=True)
   assert result.returncode==0,f'unpack {suffix} failed: '+result.stderr
   data=json.loads(result.stdout);assert data['status']=='done'
   assert mark.decode() not in result.stdout+result.stderr,'raw quarantine value was disclosed'
   expected=mark if marked else None
   real_paths=[dest,*[p for p in dest.rglob('*') if not p.name.startswith('._')]]
   assert any(p.is_file() for p in real_paths),'no output payload'
   for path in real_paths:
    assert read_optional(path)==expected,f'quarantine mismatch on {suffix} output'
    if path.is_file():assert path.read_bytes()==source.read_bytes()
   assert hashlib.sha256(archive.read_bytes()).hexdigest()==before and read_optional(archive)==before_mark,'source changed'
   if marked:
    rows={r['category']:r for r in data['observations']['categories']}
    row=rows['staging_quarantine_metadata'];assert row['verified']==len(real_paths) and row['failed']==0
    assert rows['archive_quarantine_metadata']['attempted']>=2
   print(f'unpack {suffix}: '+('marked exact' if marked else 'unmarked unchanged'),flush=True)
 # Force attribute application to fail on a genuine, unreadable extracted file.
 archive=root/'unreadable.tar'
 with tarfile.open(archive,'w') as tar:
  info=tarfile.TarInfo('locked');info.mode=0;info.size=9;tar.addfile(info,io.BytesIO(b'synthetic'))
 set_attr(archive,'com.apple.quarantine',mark)
 refused_dest=root/'refused'
 result=subprocess.run([str(binary),str(archive),'--into',str(refused_dest),'--json'],capture_output=True,text=True)
 assert result.returncode==2 and json.loads(result.stdout)['status']=='refused',result.stderr
 assert 'preserve archive quarantine' in result.stderr,'refusal did not exercise attribute preservation'
 assert not refused_dest.exists() and not list(root.glob('.refused.unpack-*.partial')),'failed preservation published or left staging'
 print('unreadable attribute target: refused, no destination, staging removed',flush=True)
 # A deliberately broken counterpart loses the mark on a bare gzip; the same check rejects it.
 control=root/'control';control.write_bytes(b'synthetic')
 assert read_optional(control)!=mark,'dirty-control unexpectedly marked'
 print('control: unmarked extracted payload rejected by exact-quarantine check',flush=True)

if __name__=='__main__':
 mode=sys.argv[1]
 if mode=='raw':
  with tempfile.TemporaryDirectory(prefix='etudes-quarantine-raw-',dir='/private/tmp') as temporary:raw(pathlib.Path(temporary))
 else:
  binary=pathlib.Path(sys.argv[2]).resolve()
  with tempfile.TemporaryDirectory(prefix='etudes-quarantine-unpack-',dir='/private/tmp') as temporary:
   root=pathlib.Path(temporary)
   destination=pathlib.Path(sys.argv[3]).resolve() if len(sys.argv)>3 else root
   unpack(root,binary,destination)
