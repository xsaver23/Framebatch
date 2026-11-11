"""Independent exported-file checks. Requires Pillow, numpy and psutil.
Run: python scripts/verify.py --exe src-tauri/target/release/framebatch-cli.exe --work ../../work/verification
"""
import argparse, hashlib, io, json, os, pathlib, sqlite3, struct, subprocess, time
import numpy as np
import psutil
from PIL import Image, ImageCms, ImageOps

def recipe(source, destination, **changes):
    r = dict(version=1, sources=[str(source)], destination=str(destination), recursive=True,
             suffix='-edited', format='jpeg', quality=92, workers=2, memory_mib=1024,
             quick=dict(levels=False,contrast=True,color=False,sharpen=False),
             size=dict(resize=True,width=6,height=4,unit='inches',constrain=True,ppi=300))
    r.update(changes)
    return r

def sha(path):
    h=hashlib.sha256()
    with open(path,'rb') as f:
        for b in iter(lambda:f.read(1024*1024),b''): h.update(b)
    return h.hexdigest()

def monitor(command, logfile, env=None):
    start=time.perf_counter(); peak=0
    with open(logfile,'w',encoding='utf-8') as log:
        p=subprocess.Popen(command,stdout=log,stderr=subprocess.STDOUT,env=env)
        root=psutil.Process(p.pid)
        while p.poll() is None:
            try:
                total=0
                for child in [root,*root.children(recursive=True)]:
                    try:
                        mi=child.memory_info(); total+=getattr(mi,'private',mi.rss)
                    except psutil.Error:pass
                peak=max(peak,total)
            except psutil.Error:pass
            time.sleep(.05)
    lines=pathlib.Path(logfile).read_text(encoding='utf-8').splitlines()
    last=next((json.loads(l) for l in reversed(lines) if l.startswith('{')),None)
    return dict(exit_code=p.returncode,elapsed_seconds=time.perf_counter()-start,peak_private_bytes=peak,final=last)

def run_job(exe,r,root,name,env=None):
    target=root/name;target.mkdir(parents=True,exist_ok=True)
    f=target/'recipe.json';f.write_text(json.dumps(r,indent=2));journals=target/'journals'
    result=monitor([str(exe),'run',str(f),str(journals)],target/'run.jsonl',env)
    result['recipe']=r;result['journal']=str(next(journals.glob('*.sqlite')))
    (target/'result.json').write_text(json.dumps(result,indent=2));return result

def create_corpus(folder,count=20,w=3000,h=2000):
    folder.mkdir(parents=True,exist_ok=True)
    profile=ImageCms.ImageCmsProfile(ImageCms.createProfile('sRGB')).tobytes()
    for i in range(count):
        iw,ih=(4000,3000) if i%2 else (w,h)
        yy,xx=np.mgrid[0:ih,0:iw]
        a=np.stack([40+(xx*170//iw),30+(yy*170//ih),50+((xx+yy)*140//(iw+ih))],axis=2).astype('uint8')
        im=Image.fromarray(a);ex=Image.Exif();ex[274]=1;ex[306]='2026:11:06 12:00:00'
        im.save(folder/f'photo-{i:04}.jpg',quality=95,dpi=(72,72),icc_profile=profile,exif=ex)

def verify_exports(source,destination,expected,ppi=300,ext='jpg'):
    files=sorted(destination.glob('*.'+ext)); assert len(files)==expected,(len(files),expected)
    checks=[]
    for file in files:
        with Image.open(file) as im:
            im.load(); density=im.info.get('dpi');ex=im.getexif()
            if not density and ex.get(296)==2:density=(float(ex[282]),float(ex[283]))
            if ppi is not None:assert density and all(abs(v-ppi)<.02 for v in density),(file,density)
            assert ex.get(274,1)==1;assert not ex.get(34853);assert im.info.get('icc_profile'),file
            checks.append(dict(file=file.name,size=im.size,density=density))
    return checks

def main():
    p=argparse.ArgumentParser();p.add_argument('--exe',type=pathlib.Path,required=True);p.add_argument('--work',type=pathlib.Path,required=True);a=p.parse_args();exe=a.exe.resolve();root=a.work.resolve();root.mkdir(parents=True,exist_ok=True)
    source=root/'source20';create_corpus(source);originals={f.name:sha(f) for f in source.glob('*.jpg')};results={}
    for name,resize in [('fit20',True),('original20',False)]:
        r=recipe(source,root/name/'output');r['size']['resize']=resize
        res=run_job(exe,r,root,name);assert res['exit_code']==0 and res['final']['completed']==20,res
        checks=verify_exports(source,pathlib.Path(r['destination']),20)
        for c in checks:
            i=int(c['file'].split('-')[1]); expected=((1600,1200) if i%2 else (1800,1200)) if resize else ((4000,3000) if i%2 else (3000,2000));assert tuple(c['size'])==expected,c
        res['independent_reader']=checks;results[name]=res
    small=root/'edge-source';small.mkdir(exist_ok=True)
    # Distinct colored corners prove orientation, rather than only swapped dimensions.
    im=Image.new('RGB',(120,80),(128,128,128)); im.paste((250,10,10),(0,0,40,40));ex=Image.Exif();ex[274]=6;im.save(small/'portrait.jpg',quality=100,exif=ex)
    alpha=Image.new('RGBA',(120,80),(30,70,120,0));alpha.paste((20,180,40,128),(20,20,70,60));alpha.save(small/'alpha.png')
    for fmt in ['jpeg','png','webp']:
        r=recipe(small,root/f'edge-{fmt}'/'output',format=fmt,quick={});r['size']['resize']=False
        res=run_job(exe,r,root,f'edge-{fmt}');assert res['final']['completed']==2,res
        checks=verify_exports(small,pathlib.Path(r['destination']),2,ext='jpg' if fmt=='jpeg' else fmt)
        with Image.open(pathlib.Path(r['destination'])/('portrait-edited.'+('jpg' if fmt=='jpeg' else fmt)))as out:assert out.size==(80,120);pix=out.convert('RGB').getpixel((60,20));assert pix[0]>200 and pix[1]<50,pix
        with Image.open(pathlib.Path(r['destination'])/('alpha-edited.'+('jpg' if fmt=='jpeg' else fmt)))as out:
            if fmt=='jpeg':assert min(out.getpixel((0,0)))>245
            else:assert out.convert('RGBA').getpixel((0,0))[3]==0;assert out.convert('RGBA').getpixel((40,40))[3]==128
        res['independent_reader']=checks;results[f'edge-{fmt}']=res
    # Duplicate basenames, corrupt files, Unicode paths, source exclusion.
    mixed=root/'mixed';(mixed/'a').mkdir(parents=True,exist_ok=True);(mixed/'b').mkdir(exist_ok=True)
    for sub in ['a','b']:Image.new('RGB',(120,80),(50,80,90)).save(mixed/sub/'旅-photo.jpg')
    (mixed/'broken.jpg').write_bytes(b'not a jpeg');out=mixed/'output';out.mkdir(exist_ok=True);sentinel=out/'旅-photo-edited.jpg';sentinel.write_bytes(b'keep existing');before=sha(sentinel)
    r=recipe(mixed,out);r['size']['resize']=False;res=run_job(exe,r,root,'mixed-run');assert res['final']['completed']==2 and res['final']['failed']==1,res;assert sha(sentinel)==before;assert len(list(out.glob('*.jpg')))==3;results['mixed']=res
    for stage in ['encoding','staged','published']:
        r=recipe(small,root/f'crash-{stage}'/'output');r['size']['resize']=False
        env=os.environ.copy();env['FRAMEBATCH_TEST_CRASH']=stage
        res=run_job(exe,r,root,'crash-'+stage,env);assert res['exit_code']==86,res
        resumed=monitor([str(exe),'resume',res['journal']],root/f'crash-{stage}'/'resume.jsonl');assert resumed['exit_code']==0 and resumed['final']['completed']==2,resumed
        verify_exports(small,pathlib.Path(r['destination']),2);assert not list(pathlib.Path(r['destination']).glob('*.part'));results['crash-'+stage]=dict(interrupted=res,resumed=resumed)
    # Read the actual exports for every sizing policy, rather than only checking calculations.
    sizes=root/'size-input';sizes.mkdir(exist_ok=True)
    for name,unit,w,h,ppi,resize,constrain,expected,index in [
      ('centimeters','cm',15.24,10.16,300,True,True,(1800,1200),0),
      ('millimeters','mm',152.4,101.6,300,True,True,(1800,1200),0),
      ('percent','percent',50,None,300,True,True,(2000,1500),1),
      ('density150','inches',6,4,150,False,True,(3000,2000),0),
      ('stretch','pixels',900,900,300,True,False,(900,900),0),
      ('portrait-width','pixels',1200,None,300,True,True,(1200,1800),2)]:
        src=source/f'photo-{index:04}.jpg'
        if index==2:
            src=sizes/'portrait.jpg'; ex=Image.Exif();ex[274]=6;Image.new('RGB',(3000,2000),(80,100,120)).save(src,exif=ex,dpi=(72,72))
        r=recipe(src,root/name/'output');r['size']=dict(resize=resize,width=w,height=h,unit=unit,constrain=constrain,ppi=ppi)
        res=run_job(exe,r,root,name);assert res['final']['completed']==1,res
        check=verify_exports(src,pathlib.Path(r['destination']),1,ppi=ppi)[0];assert tuple(check['size'])==expected,check
        results[name]=dict(result=res,check=check)
    unknown=sizes/'no-density.png';Image.new('RGB',(100,80),(90,100,110)).save(unknown)
    r=recipe(unknown,root/'missing-density'/'output');r['size']['ppi']=None
    res=run_job(exe,r,root,'missing-density');assert res['exit_code']==1 and '1 photos have no print density' in (root/'missing-density'/'run.jsonl').read_text();results['missing-density']=res
    r=recipe(unknown,root/'oversize'/'output');r['size'].update(unit='pixels',width=100000,height=100000)
    res=run_job(exe,r,root,'oversize');assert res['final']['failed']==1 and res['final']['completed']==0;results['oversize']=res
    r=recipe(unknown,root/'unspecified-density'/'output',format='png',quick={});r['size'].update(resize=False,ppi=None)
    res=run_job(exe,r,root,'unspecified-density')
    with Image.open(pathlib.Path(r['destination'])/'no-density-edited.png')as im: assert not im.info.get('dpi');assert im.getpixel((10,10))[:3]==(90,100,110)
    results['unspecified-density']=res
    # Unsupported animation, bit depth, and non-RGB profiles fail individually.
    unsupported=root/'unsupported-input';unsupported.mkdir(exist_ok=True)
    frames=[Image.new('RGB',(64,48),color)for color in ['red','blue']]
    frames[0].save(unsupported/'animated.png',save_all=True,append_images=frames[1:],duration=100,loop=0)
    frames[0].save(unsupported/'animated.webp',save_all=True,append_images=frames[1:],duration=100,loop=0)
    Image.fromarray(np.full((48,64),32000,dtype=np.uint16)).save(unsupported/'depth16.png')
    frames[0].save(unsupported/'xyz.png',icc_profile=ImageCms.ImageCmsProfile(ImageCms.createProfile('XYZ')).tobytes())
    r=recipe(unsupported,root/'unsupported'/'output');r['size']['resize']=False
    res=run_job(exe,r,root,'unsupported');assert res['final']['failed']==4 and res['final']['completed']==0,res;results['unsupported']=res
    # Custom RGB transfer curve proves that embedded color is converted, not just relabeled.
    profile=bytearray(ImageCms.ImageCmsProfile(ImageCms.createProfile('sRGB')).tobytes())
    count=struct.unpack_from('>I',profile,128)[0]
    for i in range(count):
        tag,offset,length=struct.unpack_from('>4sII',profile,132+i*12)
        if tag==b'rTRC':
            assert profile[offset:offset+4]==b'para';struct.pack_into('>i',profile,offset+12,int(1.8*65536));break
    profile[84:100]=bytes(16)
    rgb=Image.new('RGB',(120,80),(120,70,160));color=sizes/'custom-rgb.png';rgb.save(color,icc_profile=bytes(profile))
    expected=ImageCms.profileToProfile(rgb,ImageCms.ImageCmsProfile(io.BytesIO(bytes(profile))),ImageCms.createProfile('sRGB'),renderingIntent=0,outputMode='RGB')
    r=recipe(color,root/'icc-conversion'/'output',format='png',quick={});r['size']['resize']=False
    res=run_job(exe,r,root,'icc-conversion');assert res['final']['completed']==1,res
    with Image.open(pathlib.Path(r['destination'])/'custom-rgb-edited.png')as im:
        difference=np.abs(np.asarray(im.convert('RGB')).astype(int)-np.asarray(expected).astype(int));assert difference.max()<=2,difference.max()
    results['icc-conversion']=dict(result=res,maximum_channel_error=int(difference.max()),tolerance=2)
    # Keep only a whitelist of capture fields, while normalizing orientation/density and dropping GPS.
    from PIL.TiffImagePlugin import IFDRational
    capture=sizes/'capture.jpg';ex=Image.Exif();ex[271]='SONY';ex[272]='ILCE-6700';ex[274]=6
    ex[34665]={36867:'2025:04:17 14:23:45',33434:IFDRational(1,250),33437:IFDRational(28,10),34855:200}
    ex[34853]={1:'N',2:(IFDRational(40),IFDRational(1),IFDRational(1))}
    rgb.save(capture,exif=ex,dpi=(72,72))
    for fmt in ['jpeg','png','webp']:
        name='capture-'+fmt;r=recipe(capture,root/name/'output',format=fmt,quick={},retain_capture=True);r['size']['resize']=False
        res=run_job(exe,r,root,name);assert res['final']['completed']==1,res
        f=next(pathlib.Path(r['destination']).glob('*.'+('jpg'if fmt=='jpeg'else fmt)))
        with Image.open(f)as im:
            im.load();ex=im.getexif();assert ex[272]=='ILCE-6700';assert ex[274]==1;assert not ex.get(34853);assert ex.get_ifd(34665)[36867]=='2025:04:17 14:23:45';assert im.size==(80,120);assert float(ex[282])==300
        results[name]=res
    # A simulated short write must pause, preserve prior files, and recover with retry.
    r=recipe(source,root/'disk-full'/'output');env=os.environ.copy();env['FRAMEBATCH_TEST_WRITE_LIMIT']='1024';env['FRAMEBATCH_CANCEL_AFTER_MS']='1500'
    res=run_job(exe,r,root,'disk-full',env);log=(root/'disk-full'/'run.jsonl').read_text();assert 'paused' in log and res['final']['state']=='cancelled',res
    resumed=monitor([str(exe),'resume',res['journal'],'--retry'],root/'disk-full'/'resume.jsonl');assert resumed['final']['completed']==20 and resumed['final']['failed']==0,resumed
    verify_exports(source,pathlib.Path(r['destination']),20);assert not list(pathlib.Path(r['destination']).glob('*.part'));results['simulated-disk-full']=dict(interrupted=res,resumed=resumed)
    # Pause drains active workers; cancel acknowledges separately from the codec drain.
    r=recipe(source,root/'pause-cancel'/'output');env=os.environ.copy();env['FRAMEBATCH_PAUSE_AFTER_MS']='250';env['FRAMEBATCH_CONTINUE_AFTER_MS']='1500';env['FRAMEBATCH_CANCEL_AFTER_MS']='2250'
    res=run_job(exe,r,root,'pause-cancel',env);lines=(root/'pause-cancel'/'run.jsonl').read_text().splitlines();states=[json.loads(l)for l in lines if l.startswith('{')]
    paused=[p for p in states if p['state']=='paused'and p['active']==0];assert paused and len({p['completed']for p in paused})==1,states
    assert res['final']['state']=='cancelled';assert not list(pathlib.Path(r['destination']).glob('*.part'));results['pause-cancel']=res
    orientations=root/'all-orientations-input';orientations.mkdir(exist_ok=True)
    yy,xx=np.mgrid[0:16,0:24];pixels=np.stack([xx*10,yy*15,(xx+yy)*6],axis=2).astype('uint8')
    for orientation in range(1,9):
        ex=Image.Exif();ex[274]=orientation;Image.fromarray(pixels).save(orientations/f'orientation-{orientation}.png',exif=ex)
    r=recipe(orientations,root/'all-orientations'/'output',format='png',quick={});r['size']['resize']=False
    res=run_job(exe,r,root,'all-orientations');assert res['final']['completed']==8,res
    for source_file in orientations.glob('*.png'):
        with Image.open(source_file)as im,Image.open(pathlib.Path(r['destination'])/(source_file.stem+'-edited.png'))as out:
            assert np.array_equal(np.asarray(ImageOps.exif_transpose(im).convert('RGBA')),np.asarray(out)),source_file
    results['all-orientations']=res
    alpha_edge=sizes/'alpha-edge.png';im=Image.new('RGBA',(120,80),(255,0,0,0));im.paste((0,255,0,180),(30,20,90,60));im.save(alpha_edge)
    r=recipe(alpha_edge,root/'alpha-edge'/'output',format='png',quick={'sharpen':True});r['size'].update(unit='percent',width=50,height=50)
    res=run_job(exe,r,root,'alpha-edge');assert res['final']['completed']==1,res
    with Image.open(pathlib.Path(r['destination'])/'alpha-edge-edited.png')as im:
        pixels=np.asarray(im);visible=pixels[:,:,3]>32;assert pixels[:,:,0][visible].max()<5;assert pixels[:,:,2][visible].max()<5;assert pixels[:,:,1][visible].min()>245;assert pixels[0,0,3]==0
    results['alpha-edge']=res
    # Changed finished outputs are reported; retry creates new names instead of replacing them.
    journal=results['fit20']['journal'];output=root/'fit20'/'output'/'photo-0000-edited.jpg';output.write_bytes(b'external replacement');sentinel=sha(output)
    resumed=monitor([str(exe),'resume',journal],root/'changed-resume.jsonl');assert resumed['final']['completed']==19 and resumed['final']['failed']==1,resumed
    retried=monitor([str(exe),'resume',journal,'--retry'],root/'changed-retry.jsonl');assert retried['final']['completed']==20 and sha(output)==sentinel;assert (root/'fit20'/'output'/'photo-0000-edited-001.jpg').exists();results['changed-output']=dict(resumed=resumed,retried=retried)
    assert originals=={f.name:sha(f)for f in source.glob('*.jpg')},'Source files changed'
    (root/'verification.json').write_text(json.dumps(results,indent=2));print(json.dumps({k:{'seconds':v.get('elapsed_seconds'),'peak_mib':round(v.get('peak_private_bytes',0)/1048576,1)}for k,v in results.items()},indent=2));print('PASS: independent dimensions, density, alpha, orientation, collisions, corrupt inputs, crash recovery, source hashes')
if __name__=='__main__':main()
