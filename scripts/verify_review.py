"""Independent regression checks added after the Opus/Fable reviews."""
import argparse,io,json,pathlib,struct
from PIL import Image
from verify import recipe,run_job,verify_exports,sha

def main():
 p=argparse.ArgumentParser();p.add_argument('--exe',type=pathlib.Path,required=True);p.add_argument('--work',type=pathlib.Path,required=True);p.add_argument('--sony',type=pathlib.Path,required=True);a=p.parse_args();root=a.work.resolve();root.mkdir(parents=True,exist_ok=True);exe=a.exe.resolve();results={}
 # EXIF-only, unequal axes and orientation prove density is read and swapped.
 source=root/'exif-only';source.mkdir();im=Image.new('RGB',(120,80),(25,90,140));ex=Image.Exif();ex[274]=6;ex[282]=350;ex[283]=175;ex[296]=2;im.save(source/'exif.jpg',exif=ex)
 r=recipe(source,root/'exif-output',quick={});r['size'].update(resize=False,ppi=None)
 result=run_job(exe,r,root,'exif');assert result['final']['completed']==1,result
 with Image.open(root/'exif-output/exif-edited.jpg') as out:out.load();assert out.size==(80,120);assert out.info['dpi']==(175,350),out.info['dpi']
 results['exif_only_density']=result
 # The user's camera source, without changing requested print density.
 sony=next(a.sony.resolve().glob('*.JPG'));original=sha(sony);r=recipe(sony,root/'sony-output',quick={});r['size'].update(resize=False,ppi=None)
 result=run_job(exe,r,root,'sony');assert result['final']['completed']==1,result
 results['sony_original_density']=verify_exports(sony,root/'sony-output',1,ppi=350);assert sha(sony)==original
 # Valid JPEG scan data plus deliberately malformed optional EXIF.
 malformed=root/'malformed';malformed.mkdir();buffer=io.BytesIO();im.save(buffer,format='JPEG');raw=buffer.getvalue();bad=b'Exif\0\0II\x2a\x00\xff\xff\xff\x7f';(malformed/'bad-tags.jpg').write_bytes(raw[:2]+b'\xff\xe1'+struct.pack('>H',len(bad)+2)+bad+raw[2:])
 r=recipe(malformed,root/'malformed-output',retain_capture=True,quick={});r['size']['resize']=False
 result=run_job(exe,r,root,'malformed');assert result['final']['completed']==1,result
 results['malformed_optional_metadata']=verify_exports(malformed,root/'malformed-output',1)
 # Production executable ignores fault injection variables.
 import os
 env=os.environ.copy();env.update(FRAMEBATCH_TEST_CRASH='encoding',FRAMEBATCH_TEST_WRITE_LIMIT='0');r=recipe(source,root/'production-hooks-output',quick={});r['size']['resize']=False
 result=run_job(exe,r,root,'production-hooks',env);assert result['exit_code']==0 and result['final']['completed']==1,result
 results['production_has_no_test_hooks']=result
 (root/'results.json').write_text(json.dumps(results,indent=2));print('Review regression export checks passed.',flush=True)
if __name__=='__main__':main()
