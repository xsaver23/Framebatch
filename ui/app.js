'use strict';
const $ = id => document.getElementById(id);
const invoke = (...args) => window.__TAURI__.core.invoke(...args);
let recipe, sources = [], rows = [], selected = null, page = 0, revision = 0, busy = false, lastState = '', currentProgress = {}, previewTimer, pollBusy = false, scanning = false, oldUnit = 'inches', selectedInfo = null;
let preset = null, manualDefaults, scanGeneration=0, changedAt=0, starting=false;
let errorsPage=0, closingAfterCancel=false, allowClose=false, announcedState='', announcedBucket=-1;
const previewSamples=[];
let thumbnailBusy=false, previewRequests=0;
const thumbnailQueue=new Map();
const thumbnailObserver=new IntersectionObserver(entries=>{for(const entry of entries){if(entry.isIntersecting&&!entry.target.dataset.loaded)thumbnailQueue.set(entry.target,entry.target.dataset.path);else thumbnailQueue.delete(entry.target);}pumpThumbnails();},{root:$('filmstrip'),rootMargin:'40px'});
async function pumpThumbnails(){
  if(thumbnailBusy)return;
  thumbnailBusy=true;
  try{while(thumbnailQueue.size){if(previewRequests){setTimeout(pumpThumbnails,100);break;}
    const [button,path]=thumbnailQueue.entries().next().value;thumbnailQueue.delete(button);
    if(!button.isConnected)continue;
    try{const src=await invoke('thumbnail',{path});if(button.isConnected){button.querySelector('img').src=src;button.dataset.loaded='true';}}catch{button.dataset.loaded='true';}
  }}finally{thumbnailBusy=false;}
}
const manualControls = [['exposure','Exposure',-4,4,.1],['contrast','Contrast',-100,100,1],['shadows','Shadows',-100,100,1],['highlights','Highlights',-100,100,1],['saturation','Saturation',-100,100,1],['warmth','Warmth',-100,100,1],['sharpening','Sharpening',0,100,1],['black_point','Black point',0,254,1],['white_point','White point',1,255,1],['gamma','Gamma',.1,5,.05]];
function showError(e) { const el=$('error');el.hidden=!e;if(el.textContent!==String(e||''))el.textContent=String(e||''); }
function guard(fn) { return async (...args) => { try { showError(''); await fn(...args); } catch(e) { showError(e); } }; }
function readRecipe() {
  const r = structuredClone(recipe);
  r.sources = $('source-mode').value === 'folder' ? [$('source').value.trim()].filter(Boolean) : sources;
  r.destination = $('destination').value.trim(); r.recursive = $('recursive').checked;
  r.suffix = $('suffix').value; r.format = $('format').value; r.quality = Number($('quality').value);
  r.workers = Number($('workers').value); r.memory_mib = Number($('memory').value);
  r.retain_capture = $('retain-capture').checked;
  r.size = {resize:$('resize').checked,width:$('width').value === '' ? null : Number($('width').value),height:$('height').value === '' ? null : Number($('height').value),unit:$('unit').value,constrain:$('fit').value!=='stretch',fit:$('fit').value==='stretch'?'fit':$('fit').value,match_orientation:$('match-orientation').checked,ppi:$('density-mode').value === 'set' ? Number($('ppi').value) : null};
  for (const key of ['levels','contrast','color','sharpen']) r.quick[key] = $(key).checked;
  r.preset = preset; r.adjust = structuredClone(adjust);
  for (const [key,name] of manualControls) {const input=$('manual-'+key).nextElementSibling;if(input.value===''||!input.checkValidity())throw Error(`Enter a valid ${name.toLowerCase()} value (${input.min} to ${input.max}, step ${input.step}).`);r.manual[key]=Number(input.value);}
  return r;
}
function syncControls() {
  const resize = $('resize').checked;
  for (const id of ['width','height','unit','fit','match-orientation']) $(id).disabled = !resize || busy;
  $('ppi').disabled = $('density-mode').value !== 'set' || busy;
  $('recursive').disabled = busy;
  $('source').readOnly = $('source-mode').value === 'files';
  $('quality-label').hidden = $('format').value !== 'jpeg';
  $('name-example').textContent = `Example: DSC01234${$('suffix').value}.${$('format').value === 'jpeg' ? 'jpg' : $('format').value}`;
  const metadataNote=$('retain-capture').checked?'Capture details kept; GPS removed':'Camera and GPS tags removed';
  $('format-note').textContent=$('format').value==='webp'?`Lossless; alpha kept. ${metadataNote}. Print apps may ignore WebP density; use JPEG or PNG for printing.`:`sRGB color; ${$('format').value==='jpeg'?'white matte':'alpha kept'}. ${metadataNote}.`;
  const fitHelp={fit:'The whole photo fits inside the box; one side may come out shorter.',fill:'Every photo is exactly the box size; the edges of the longer side are cropped from the center.',border:'Every photo is exactly the box size; the whole photo is kept and white margins fill the rest.',stretch:'Every photo is exactly the box size; photos of another shape are stretched.'};
  const turned=$('match-orientation').checked&&$('unit').value!=='percent'?' The box turns for landscape photos, so 5 × 7 becomes 7 × 5.':'';
  $('size-help').textContent = resize ? ($('width').value===''||$('height').value===''?'Only one dimension is set: the other follows the photo’s shape.':fitHelp[$('fit').value]+turned) : 'Resize is off: pixel dimensions stay unchanged. PPI is the number of image pixels per printed inch.';
  $('preview-open').disabled = !selected;
  $('scan').disabled=starting||busy||scanning;$('resume').disabled=starting||busy||scanning;
  $('process').disabled = starting || scanning || busy || !['ready','complete','cancelled'].includes(currentProgress.state) || !currentProgress.discovered || !$('destination').value.trim();
  $('previous').disabled=page===0; $('next').disabled=(page+1)*50>=currentProgress.discovered;
  $('preview-only').hidden=!busy;
  syncStage();
}
// Empty state, page buttons and photo arrows of the central preview.
function syncStage() {
  const has=!!selected, discovered=currentProgress.discovered||0;
  $('stage').classList.toggle('has-photo',has);$('stage-empty').hidden=has;$('comparison').hidden=!has;
  $('empty-browse').disabled=starting||busy||scanning;$('previous').parentElement.hidden=discovered<=50;
  if(has)return;
  const looking=scanning||['scanning','preparing'].includes(currentProgress.state), none=!looking&&currentProgress.state==='ready'&&!discovered&&$('source').value.trim();
  const title=looking?'Looking for photos…':none?'No supported photos found':'Choose photos to get started';
  const text=looking?'Thumbnails appear below as soon as the folder has been read.':none?'Check “Include all subfolders” if the photos are in subfolders. RAW files are not supported.':'Pick a folder of JPEG, PNG or WebP photos, or drop photos or a folder anywhere in this window.';
  if($('empty-title').textContent!==title){$('empty-title').textContent=title;$('empty-text').textContent=text;}
}
function changed() { ++revision; changedAt=performance.now(); syncControls(); markModified(); if (selected) { clearTimeout(previewTimer); previewTimer = setTimeout(() => refreshPreview().catch(showError),16); } }
async function scan() {
  if (busy || scanning) return;
  const scanRecipe=readRecipe();
  scanning = true; ++scanGeneration; ++revision; selected = null; selectedInfo = null; page = 0; rows = []; $('filmstrip').replaceChildren(); $('selected-name').textContent='No photo selected'; $('preview-caption').textContent=''; $('summary').textContent = 'Scanning photos…';
  currentProgress.state='scanning';syncControls();
  try { await invoke('scan',{recipe:scanRecipe}); lastState=''; } catch(e){currentProgress.state='empty';lastState='';$('summary').textContent='Choose a source and destination, then scan again.';throw e;} finally { scanning = false;syncControls(); }
}
async function browseSource() {
  const folder = $('source-mode').value === 'folder';
  const picked = await window.__TAURI__.dialog.open({directory:folder,multiple:!folder,title:folder?'Choose source folder':'Choose photos',filters:folder?[]:[{name:'Photos',extensions:['jpg','jpeg','png','webp']}]});
  if (!picked) return; sources = Array.isArray(picked) ? picked : [picked]; $('source').value = folder?sources[0]:`${sources.length} selected files`;
  if (!$('destination').value) { const root = folder?sources[0]:sources[0].replace(/[\\/][^\\/]+$/,''); $('destination').value = root+'\\Framebatch output'; }
  await scan();
}
async function loadRows(selectLast=false) {
  const generation=scanGeneration, requestedPage=page;
  const loaded = await invoke('files',{offset:page*50,failed:false});
  if(generation!==scanGeneration || requestedPage!==page)return;
  rows=loaded; thumbnailObserver.disconnect();thumbnailQueue.clear();const container = $('filmstrip');container.replaceChildren();
  for (const row of rows) { const b=document.createElement('button'); const name=row.path.split(/[\\/]/).pop();const img=document.createElement('img');img.alt='';const label=document.createElement('span');label.textContent=name;b.append(img,label);b.dataset.path=row.path;b.title=row.path;b.tabIndex=selected?.id===row.id?0:-1;b.setAttribute('aria-pressed',String(selected?.id===row.id));b.onclick=guard(async()=>{selected=row;await selectPhoto();});container.append(b); }
  $('page-label').textContent=rows.length?`${page*50+1}–${page*50+rows.length}`:''; $('previous').disabled=page===0; $('next').disabled=(page+1)*50>=currentProgress.discovered;
  for(const button of container.children)thumbnailObserver.observe(button);
  if (!selected && rows.length) { selected=selectLast?rows[rows.length-1]:rows[0];selectPhoto().catch(showError); }
  if (container.children.length && !container.querySelector('[tabindex="0"]')) container.children[0].tabIndex=0;
  if(!rows.length)$('summary').textContent='No JPEG, PNG or WebP photos found. Check Include all subfolders if needed. RAW files are not supported.';
}
// One photo back or forward; crosses 50-photo pages. Used by the preview arrows and the
// filmstrip's arrow keys (which move focus as soon as the selection changes, before the
// preview finishes).
async function stepPhoto(delta,focusStrip=false) {
  const i=rows.findIndex(r=>r.id===selected?.id);let preview;
  if(delta<0){if(i>0){selected=rows[i-1];preview=selectPhoto();}else if(page>0){page--;selected=null;await loadRows(true);}}
  else if(i+1<rows.length){selected=rows[i+1];preview=selectPhoto();}else if((page+1)*50<currentProgress.discovered){page++;selected=null;await loadRows();}
  if(focusStrip)$('filmstrip').querySelector('[tabindex="0"]')?.focus();
  await preview;
}
async function selectPhoto() {
  $('selected-name').textContent=selected.path.split(/[\\/]/).pop();$('selected-name').title=selected.path; syncControls();
  [...$('filmstrip').children].forEach((b,i)=>b.setAttribute('aria-pressed',String(rows[i].id===selected.id)));
  [...$('filmstrip').children].forEach((b,i)=>b.tabIndex=rows[i].id===selected.id?0:-1);
  $('filmstrip').children[rows.findIndex(r=>r.id===selected.id)]?.scrollIntoView({block:'nearest',inline:'nearest'});
  await refreshPreview();
}
async function refreshPreview() {
  if (!selected) return;const rev=++revision;const path=selected.path;const began=changedAt||performance.now();changedAt=0;++previewRequests;
  showError('');$('stage').classList.add('loading');
  try { const p=await invoke('preview',{path,recipe:readRecipe(),revision:rev,detail:$('detail').checked}); if (rev!==revision || selected?.path!==path) return;
    selectedInfo=p.info; $('before').src=p.before;$('after').src=p.after;
    const s=p.size; const density=s.ppi ? ` · ${Number(s.ppi[0].toFixed(2))} PPI · ${(s.width/s.ppi[0]).toFixed(2)} × ${(s.height/s.ppi[1]).toFixed(2)} inches` : ' · Print density unspecified';
    const description=`${s.width} × ${s.height} px${density}`;$('summary').textContent=description;$('preview-caption').textContent='Output: '+description;
    await Promise.all([$ ('before').decode(),$('after').decode()]);
    for(const id of ['before','after'])$(id).style.width=$('detail').checked?`${$(id).naturalWidth/devicePixelRatio}px`:'';
    await new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)));
    if(rev!==revision)return;
    const elapsed=Math.round(performance.now()-began);previewSamples.push(elapsed);if(previewSamples.length>100)previewSamples.shift();
    const sorted=[...previewSamples].sort((a,b)=>a-b);$('preview-time').textContent=`Preview ready in ${elapsed} ms`;$('preview-time').title=`Recent p95 ${sorted[Math.ceil(sorted.length*.95)-1]} ms (${sorted.length} samples)`;
  } catch(e) {if (rev!==revision || String(e).includes('Superseded'))return; $('summary').textContent=String(e);throw e;}finally{--previewRequests;if(!previewRequests)$('stage').classList.remove('loading');pumpThumbnails();}
}
async function poll() {
  if (pollBusy) return; pollBusy=true;
  try { let p=await invoke('progress');if(!$('source').value.trim()&&!['running','paused','recovering','scanning','preparing','cancelling'].includes(p.state))p={...p,state:'empty',discovered:0,completed:0,failed:0};currentProgress=p; const active=['scanning','preparing','running','paused','cancelling','recovering'].includes(p.state); busy=active;
    if(p.state!==lastState){ for(const el of $('settings').querySelectorAll('input,select,button')) el.disabled=active; for(const el of $('manual-details').querySelectorAll('input,button,select'))el.disabled=false; for(const el of $('stage').querySelectorAll('input,button,select'))el.disabled=false; $('preview-open').disabled=!selected; for(const el of $('filmstrip').querySelectorAll('button'))el.disabled=false;lastState=p.state;if(p.state==='ready'||(!selected&&p.discovered&&p.scan_complete))await loadRows(); }
    $('count').textContent=p.discovered?`${p.discovered.toLocaleString()} photos${p.scan_complete?' ready':' found…'}`:'No photos selected';
    $('progress-bar').hidden=!active && !['complete','cancelled'].includes(p.state);$('progress-bar').max=Math.max(1,p.discovered);$('progress-bar').value=p.completed+p.failed;
    $('status-text').textContent=p.state==='empty'?'Choose a source and destination to begin.':p.state==='scan_cancelled'?'Scan cancelled. Scan again to discover the whole folder.':p.state==='ready'?(p.discovered?`${p.discovered.toLocaleString()} photos ready. Choose your settings, then Process Photos.${p.message?' '+p.message:''}`:'No supported photos found. Check the folder and Include all subfolders; RAW is not supported.'):`${p.state.charAt(0).toUpperCase()+p.state.slice(1)} — ${p.completed.toLocaleString()} saved, ${p.failed} failed${p.active?`, ${p.active} processing`:''} · ${(p.elapsed_ms/1000).toFixed(1)} seconds${p.message?' · '+p.message:''}`;
    const bucket=Math.floor((p.completed+p.failed)/Math.max(1,p.discovered)*10);if(p.state!==announcedState||bucket!==announcedBucket){$('status-live').textContent=`${p.state.replace('_',' ')}. ${p.completed} saved, ${p.failed} failed.`;announcedState=p.state;announcedBucket=bucket;}
    $('pause').hidden=!['running','paused'].includes(p.state);$('pause').textContent=p.state==='paused'?'Continue':'Pause'; $('cancel').disabled=p.state==='cancelling';$('cancel').textContent='Cancel batch';$('cancel').hidden=!active;
    $('open-output').hidden=p.completed===0;$('resume-current').hidden=p.state!=='cancelled';$('retry').hidden=active||p.failed===0;$('show-errors').hidden=p.failed===0;
    if(p.state==='error')showError(p.message);if(['scan_cancelled','error'].includes(p.state))$('summary').textContent=p.message||'Scan stopped. Choose a source and scan again.';syncControls();
    if(closingAfterCancel&&!active){allowClose=true;await window.__TAURI__.window.getCurrentWindow().close();}
  } catch(e) {showError(e);} finally {pollBusy=false;}
}
function convertUnits() {
  const unit=$('unit').value;const ppi=$('density-mode').value==='set'?[Number($('ppi').value),Number($('ppi').value)]:selectedInfo?.ppi;
  const original=[selectedInfo?.width,selectedInfo?.height];
  function factor(u,axis){if(u==='pixels')return 1;if(u==='percent'){if(!original[axis])throw Error('Select a photo before converting percent units.');return original[axis]/100;}if(!ppi?.[axis])throw Error('Set a print resolution before converting physical units.');return ppi[axis]/(u==='cm'?2.54:u==='mm'?25.4:1);}
  try{const converted=['width','height'].map((id,i)=>$(id).value===''?'':Number((Number($(id).value)*factor(oldUnit,i)/factor(unit,i)).toPrecision(12)));for(const[i,id]of ['width','height'].entries())$(id).value=converted[i];$('unit-note').hidden=true;}catch{$('unit-note').textContent='Units changed; width and height were kept because no conversion reference is available. Enter the dimensions you want.';$('unit-note').hidden=false;}oldUnit=unit;showError('');changed();
}
async function init() {
  revision=await invoke('preview_revision');recipe=await invoke('defaults');manualDefaults=structuredClone(recipe.manual);
  adjustDefaults=structuredClone(recipe.adjust);adjust=structuredClone(adjustDefaults);builtins=await invoke('builtin_presets');
  $('workers').value=String(recipe.workers);$('memory').value=String(recipe.memory_mib);
  buildAdjust();refreshAdjust();setupLayout();
  $('browse-source').onclick=guard(browseSource);$('browse-destination').onclick=guard(async()=>{const p=await window.__TAURI__.dialog.open({directory:true,title:'Choose output folder'});if(p){$('destination').value=p;if($('source').value)await scan();}});
  $('scan').onclick=guard(scan);for(const id of ['source','destination','recursive'])$(id).addEventListener('change',guard(async()=>{syncControls();if($('source').value)await scan();}));
  for(const el of $('settings').querySelectorAll('input,select'))if(!el.closest('#stage')&&!el.id.startsWith('manual-')&&!el.id.startsWith('adjust-')&&!['unit','preset-file','preset-select'].includes(el.id))el.addEventListener('change',changed);
  $('source-mode').onchange=()=>{sources=[];$('source').value='';selected=null;selectedInfo=null;++scanGeneration;++revision;$('filmstrip').replaceChildren();$('selected-name').textContent='No photo selected';$('preview-caption').textContent='';$('summary').textContent='Choose a source folder or selected files.';currentProgress={state:'empty',discovered:0};syncControls();};
  $('unit').onchange=convertUnits;
  $('filmstrip').onkeydown=guard(async event=>{if(!['ArrowLeft','ArrowRight','ArrowUp','ArrowDown'].includes(event.key))return;event.preventDefault();await stepPhoto(['ArrowLeft','ArrowUp'].includes(event.key)?-1:1,true);});
  $('preview-previous').onclick=guard(()=>stepPhoto(-1));
  $('preview-next').onclick=guard(()=>stepPhoto(1));
  for(const fig of $('comparison').querySelectorAll('figure'))fig.onscroll=()=>{const other=[...$('comparison').querySelectorAll('figure')].find(x=>x!==fig);if(other.scrollLeft!==fig.scrollLeft)other.scrollLeft=fig.scrollLeft;if(other.scrollTop!==fig.scrollTop)other.scrollTop=fig.scrollTop;};
  await window.__TAURI__.window.getCurrentWindow().onCloseRequested(event=>{if(busy&&!allowClose){event.preventDefault();$('close-dialog').showModal();}});
  $('keep-processing').onclick=()=>$('close-dialog').close();
  $('cancel-close').onclick=guard(async()=>{closingAfterCancel=true;$('cancel-close').disabled=true;$('close-note').textContent='Waiting for active work to stop…';await invoke('control',{action:'cancel'});});
  $('auto-fix').onclick=()=>{for(const k of ['levels','contrast','color','sharpen'])$(k).checked=true;changed();};$('clear-fixes').onclick=()=>{for(const k of ['levels','contrast','color','sharpen'])$(k).checked=false;changed();};
  $('preset-choose').onclick=()=>$('preset-file').click();
  $('preset-file').onchange=guard(async()=>{const file=$('preset-file').files[0];$('preset-file').value='';if(!file)return;if(file.size>2*1024*1024)throw Error('This file is too large to be a Lightroom preset.');preset=await invoke('parse_preset',{text:await file.text(),fileName:file.name});showPreset();changed();});
  $('preset-select').onchange=()=>{const v=$('preset-select').value;if(v==='none')preset=null;else if(v!=='file')preset=structuredClone(builtins[Number(v.slice(1))]);showPreset();changed();};
  showPreset();
  $('reset-manual').onclick=()=>{for(const[k]of manualControls){$('manual-'+k).value=manualDefaults[k];$('manual-'+k).nextElementSibling.value=manualDefaults[k];}adjust=structuredClone(adjustDefaults);refreshAdjust();changed();};
  $('process').onclick=guard(async()=>{if(starting)return;starting=true;$('control-time').textContent='';syncControls();try{if(currentProgress.state!=='ready'){await scan();do{await poll();if(['error','cancelled','scan_cancelled'].includes(currentProgress.state))throw Error('Scan did not complete.');if(currentProgress.state!=='ready')await new Promise(resolve=>setTimeout(resolve,50));}while(currentProgress.state!=='ready');}await invoke('start',{recipe:readRecipe()});await poll();}finally{starting=false;syncControls();}});
  $('cancel').onclick=guard(async()=>{if(busy){const t=performance.now();await invoke('control',{action:'cancel'});$('control-time').textContent=`Cancel acknowledged in ${Math.round(performance.now()-t)} ms. Waiting for active work to stop.`;}else{await window.__TAURI__.window.getCurrentWindow().close();}});
  $('pause').onclick=guard(async()=>{const action=currentProgress.state==='paused'?'continue':'pause';const t=performance.now();await invoke('control',{action});$('control-time').textContent=`${action==='pause'?'Pause':'Continue'} acknowledged in ${Math.round(performance.now()-t)} ms.`;await poll();});
  $('open-output').onclick=guard(()=>invoke('open_output'));
  $('resume-current').onclick=guard(async()=>{await invoke('resume',{journal:currentProgress.journal,retry:false});restoreRecipe(await invoke('current_recipe'));lastState='';await poll();});
  $('retry').onclick=guard(async()=>{await invoke('resume',{journal:currentProgress.journal,retry:true});await poll();});
  $('show-errors').onclick=guard(async()=>{$('error-report').hidden=!$('error-report').hidden;errorsPage=0;await loadErrors();});$('errors-previous').onclick=guard(async()=>{errorsPage=Math.max(0,errorsPage-1);await loadErrors();});$('errors-next').onclick=guard(async()=>{errorsPage++;await loadErrors();});
  // Page buttons show the previous/next 50 photos and select the first one shown, so the
  // selection, pressed thumbnail and keyboard entry point stay on the visible page.
  $('previous').onclick=guard(async()=>{if(page===0)return;page--;selected=null;await loadRows();});
  $('next').onclick=guard(async()=>{if((page+1)*50>=currentProgress.discovered)return;page++;selected=null;await loadRows();});
  $('empty-browse').onclick=guard(browseSource);
  window.addEventListener('keydown',event=>{if(event.key==='F5'||((event.ctrlKey||event.metaKey)&&event.key.toLowerCase()==='r'))event.preventDefault();if(event.key==='Escape'&&document.body.classList.contains('focus')&&!$('close-dialog').open)setFocus(false);});
  $('detail').onchange=()=>{$('comparison').classList.toggle('detail',$('detail').checked);changed();};
  const dismissed=loadDismissed();const jobs=(await invoke('recoverable')).filter(job=>!dismissed.has(job.journal));
  function showRecovery(){const job=jobs[0];$('recovery').hidden=!job;if(job){$('recovery-note').textContent=`${job.source} → ${job.destination} · ${job.completed}/${job.discovered} saved, ${job.failed} failed`;$('recovery-note').title=$('recovery-note').textContent;}}
  showRecovery();$('dismiss-recovery').onclick=()=>{const job=jobs.shift();if(job){dismissed.add(job.journal);saveDismissed(dismissed);}showRecovery();};
  $('resume').onclick=guard(async()=>{const job=jobs[0];await invoke('resume',{journal:job.journal,retry:false});restoreRecipe(await invoke('current_recipe'));$('recovery').hidden=true;lastState='';await poll();});
  await window.__TAURI__.event.listen('tauri://drag-drop',guard(async event=>{if(busy)return;sources=event.payload.paths;$('source-mode').value='files';$('source').value=`${sources.length} dropped paths`;if(!$('destination').value){syncControls();$('summary').textContent='Choose a destination, then scan the dropped photos.';return;}await scan();}));
  if((await invoke('progress')).state!=='empty')restoreRecipe(await invoke('current_recipe'));syncControls();await poll();setInterval(poll,300);
}
// Dismissed recovery jobs. Storage may be blocked or hold bad data: fall back to an empty
// list, and keep dismissals in memory for this session if saving fails.
function loadDismissed(){try{const saved=JSON.parse(localStorage.getItem('dismissed-jobs')||'[]');return new Set(Array.isArray(saved)?saved.filter(x=>typeof x==='string'):[]);}catch{return new Set();}}
function saveDismissed(dismissed){try{localStorage.setItem('dismissed-jobs',JSON.stringify([...dismissed]));}catch{/* kept in memory only */}}
async function loadErrors(){const errors=await invoke('files',{offset:errorsPage*50,failed:true});$('errors').textContent=errors.map(x=>x.path+': '+x.error).join('\n');$('errors-page').textContent=`${errorsPage*50+1}–${errorsPage*50+errors.length} of ${currentProgress.failed}`;$('errors-previous').disabled=errorsPage===0;$('errors-next').disabled=(errorsPage+1)*50>=currentProgress.failed;}
// Adjustment panel. Manual sliders (`recipe.manual`) and the manual versions of every preset
// feature (`adjust`, same shape as an imported preset, run by the same engine right after
// the preset) are grouped the way Lightroom groups them. Only the layout changed; the
// recipe fields and processing order are as before.
let adjust, adjustDefaults, hslBand=3, gradeRegion='midtone', builtins=[];
const adjustControls=[], sliderRows=[], choices=[];
const bandNames=['Red','Orange','Yellow','Green','Aqua','Blue','Purple','Magenta'];
const bandHues=[0,30,55,120,180,225,270,310];
const gradeRegions=[['shadow','Shadows'],['midtone','Midtones'],['highlight','Highlights'],['global','Global']];
const hsl=(h,s,l)=>`hsl(${h},${s}%,${l}%)`;
const gradient=(...stops)=>`linear-gradient(90deg,${stops.join(',')})`;
const tracks={
  exposure:gradient('#26262a','#ececf0'),warmth:gradient('#4f7fd8','#9a9aa0','#e2b347'),tint:gradient('#4caf5a','#9a9aa0','#c95bc9'),
  saturation:gradient(hsl(0,0,55),hsl(350,78,58),hsl(45,90,55)),vibrance:gradient(hsl(0,0,55),hsl(350,78,58),hsl(45,90,55)),
  'grade-hue':gradient(...[0,60,120,180,240,300,360].map(h=>hsl(h,75,55))),'grade-lum':gradient('#26262a','#ececf0'),
  'cal-0':gradient('#4caf5a','#9a9aa0','#c95bc9'),'cal-1':gradient(hsl(-35,75,55),hsl(0,75,55),hsl(35,75,55)),'cal-2':gradient(hsl(0,0,55),hsl(0,80,55)),
  'cal-3':gradient(hsl(85,65,48),hsl(120,65,48),hsl(155,65,48)),'cal-4':gradient(hsl(120,0,50),hsl(120,70,45)),
  'cal-5':gradient(hsl(195,70,55),hsl(225,70,58),hsl(260,70,60)),'cal-6':gradient(hsl(225,0,55),hsl(225,75,58)),'vignette-0':gradient('#111','#777','#eee'),
};
function sliderRow(parent,id,name,min,max,step,isDefault,reset){
  const row=document.createElement('label');row.className='slider-row';const label=document.createElement('span');label.textContent=name;label.title='Double-click to reset';
  const slider=document.createElement('input');slider.type='range';slider.min=min;slider.max=max;slider.step=step;slider.id=id;slider.setAttribute('aria-label',name);
  const number=document.createElement('input');number.type='number';number.min=min;number.max=max;number.step=step;number.setAttribute('aria-label',name+' value');
  label.ondblclick=()=>{if(slider.disabled)return;slider.value=reset();slider.dispatchEvent(new Event('input'));};
  row.append(label,slider,number);parent.append(row);sliderRows.push({row,slider,isDefault});return [slider,number];
}
function manualSlider(parent,key){
  const [,name,min,max,step]=manualControls.find(c=>c[0]===key);
  const [slider,number]=sliderRow(parent,'manual-'+key,name,min,max,step,()=>Number(slider.value)===manualDefaults[key],()=>manualDefaults[key]);
  slider.value=recipe.manual[key];number.value=slider.value;if(tracks[key])slider.style.setProperty('--track-fill',tracks[key]);
  slider.oninput=()=>{number.value=slider.value;changed();};number.oninput=()=>{if(number.value!==''&&number.checkValidity())slider.value=number.value;changed();};number.onchange=()=>{if(number.value!==''&&number.checkValidity())number.value=slider.value;changed();};
}
function adjustSlider(parent,id,name,min,max,step,get,set){
  const [slider,number]=sliderRow(parent,'adjust-'+id,name,min,max,step,()=>Number(slider.value)===get(adjustDefaults),()=>get(adjustDefaults));
  number.id='adjust-'+id+'-value';if(tracks[id])slider.style.setProperty('--track-fill',tracks[id]);
  const apply=v=>{set(adjust,v);changed();};
  slider.oninput=()=>{number.value=slider.value;apply(Number(slider.value));};
  number.oninput=()=>{if(number.value!==''&&number.checkValidity()){slider.value=number.value;apply(Number(number.value));}};
  number.onchange=()=>{number.value=slider.value;};
  adjustControls.push({slider,number,get});
}
function adjustGroup(title,open=false){const d=document.createElement('details');d.className='adjust-group';d.open=open;const s=document.createElement('summary');s.textContent=title;d.append(s);$('adjust').append(d);return d;}
// A choice (color band, grading wheel) shown as swatches or a segmented control. The hidden
// <select> keeps the value, so keyboard, scripts and tests can still set it directly.
function adjustChoice(parent,id,label,options,onchange,kind,edited){
  const select=document.createElement('select');select.id='adjust-'+id;select.setAttribute('aria-label',label);select.hidden=true;
  for(const [value,text] of options)select.add(new Option(text,value));select.onchange=()=>{onchange(select.value);refreshAdjust();};
  const box=document.createElement('div');box.className=kind==='swatches'?'swatches':'segmented choice-segmented';box.setAttribute('role','group');box.setAttribute('aria-label',label);
  const buttons=options.map(([value,text],i)=>{const b=document.createElement('button');b.type='button';b.dataset.value=value;b.setAttribute('aria-label',text);b.title=text;
    if(kind==='swatches'){b.className='swatch';b.style.setProperty('--sw',hsl(bandHues[i],i===3||i===4?50:72,i===2?50:55));const name=document.createElement('span');name.className='swatch-label';name.textContent=text;b.append(name);}else b.textContent=text;
    b.onclick=()=>{select.value=value;select.dispatchEvent(new Event('change'));};box.append(b);return b;});
  parent.append(select,box);choices.push({select,buttons,edited});return select;
}
function buildAdjust(){
  let g=adjustGroup('Light',true);
  for(const k of ['exposure','contrast','highlights','shadows'])manualSlider(g,k);
  for(const [id,name] of [['whites','Whites'],['blacks','Blacks']])adjustSlider(g,id,name,-100,100,1,a=>a[id],(a,v)=>{a[id]=v;});
  g=adjustGroup('Color',true);
  const bw=document.createElement('label');bw.className='check switch-row';const box=document.createElement('input');box.type='checkbox';box.id='adjust-bw';box.className='switch';
  box.onchange=()=>{adjust.grayscale=box.checked;changed();};bw.append(box,'Black & white');g.append(bw);
  manualSlider(g,'warmth');adjustSlider(g,'tint','Tint',-100,100,1,a=>a.tint,(a,v)=>{a.tint=v;});
  adjustSlider(g,'vibrance','Vibrance',-100,100,1,a=>a.vibrance,(a,v)=>{a.vibrance=v;});manualSlider(g,'saturation');
  g=adjustGroup('Levels');
  for(const k of ['black_point','white_point','gamma'])manualSlider(g,k);
  g=adjustGroup('Detail');manualSlider(g,'sharpening');
  g=adjustGroup('Tone curve');
  ['Shadows','Darks','Lights','Highlights'].forEach((name,i)=>adjustSlider(g,'curve-'+i,name,-100,100,1,a=>a.parametric[i],(a,v)=>{a.parametric[i]=v;}));
  g=adjustGroup('Color mixer');
  adjustChoice(g,'band','Color',bandNames.map((n,i)=>[String(i),n]),v=>{hslBand=Number(v);},'swatches',v=>adjust.hsl[Number(v)].some(x=>x!==0)).value=String(hslBand);
  ['Hue','Saturation','Luminance'].forEach((name,k)=>adjustSlider(g,'hsl-'+k,name,-100,100,1,a=>a.hsl[hslBand][k],(a,v)=>{a.hsl[hslBand][k]=v;}));
  g=adjustGroup('Color grading');
  adjustChoice(g,'region','Wheel',gradeRegions,v=>{gradeRegion=v;},'segmented',v=>['hue','sat','lum'].some(k=>adjust.grading[v+'_'+k]!==adjustDefaults.grading[v+'_'+k])).value=gradeRegion;
  adjustSlider(g,'grade-hue','Hue',0,360,1,a=>a.grading[gradeRegion+'_hue'],(a,v)=>{a.grading[gradeRegion+'_hue']=v;});
  adjustSlider(g,'grade-sat','Saturation',0,100,1,a=>a.grading[gradeRegion+'_sat'],(a,v)=>{a.grading[gradeRegion+'_sat']=v;});
  adjustSlider(g,'grade-lum','Luminance',-100,100,1,a=>a.grading[gradeRegion+'_lum'],(a,v)=>{a.grading[gradeRegion+'_lum']=v;});
  adjustSlider(g,'grade-balance','Balance',-100,100,1,a=>a.grading.balance,(a,v)=>{a.grading.balance=v;});
  adjustSlider(g,'grade-blending','Blending',0,100,1,a=>a.grading.blending,(a,v)=>{a.grading.blending=v;});
  g=adjustGroup('Effects');
  adjustSlider(g,'glow','Glow',0,100,1,a=>a.glow??0,(a,v)=>{a.glow=v;});
  [['Vignette',-100,100],['Midpoint',0,100],['Feather',0,100],['Roundness',-100,100]].forEach(([name,min,max],i)=>adjustSlider(g,'vignette-'+i,name,min,max,1,a=>a.vignette[i],(a,v)=>{a.vignette[i]=v;}));
  [['Grain',0,100],['Grain size',0,100],['Roughness',0,100]].forEach(([name,min,max],i)=>adjustSlider(g,'grain-'+i,name,min,max,1,a=>a.grain[i],(a,v)=>{a.grain[i]=v;}));
  g=adjustGroup('Calibration');
  ['Shadow tint','Red hue','Red saturation','Green hue','Green saturation','Blue hue','Blue saturation'].forEach((name,i)=>adjustSlider(g,'cal-'+i,name,-100,100,1,a=>a.calibration[i],(a,v)=>{a.calibration[i]=v;}));
}
function gradeSatTrack(){const h=adjust.grading[gradeRegion+'_hue'];$('adjust-grade-sat').style.setProperty('--track-fill',gradient(hsl(h,0,55),hsl(h,80,55)));}
function refreshAdjust(){
  for(const c of adjustControls){const v=c.get(adjust);c.slider.value=v;c.number.value=v;}
  $('adjust-bw').checked=adjust.grayscale;
  const h=bandHues[hslBand];
  $('adjust-hsl-0').style.setProperty('--track-fill',gradient(hsl(h-40,70,52),hsl(h,70,52),hsl(h+40,70,52)));
  $('adjust-hsl-1').style.setProperty('--track-fill',gradient(hsl(h,0,52),hsl(h,85,52)));
  $('adjust-hsl-2').style.setProperty('--track-fill',gradient(hsl(h,60,16),hsl(h,60,84)));
  markModified();
}
// Highlights changed sliders, bands and wheels so edits are easy to find.
function markModified(){
  if(!adjust)return;
  for(const {row,isDefault} of sliderRows)row.classList.toggle('modified',!isDefault());
  for(const {select,buttons,edited} of choices)for(const b of buttons){b.setAttribute('aria-pressed',String(b.dataset.value===select.value));b.classList.toggle('edited',edited(b.dataset.value));}
  gradeSatTrack();
}
// Window chrome: theme, preview view, focus mode and filmstrip scrolling.
function applyTheme(theme){
  document.documentElement.dataset.theme=theme;try{localStorage.setItem('theme',theme);}catch{/* storage unavailable */}
  const next=theme==='dark'?'light':'dark',button=$('theme-toggle');
  button.setAttribute('aria-label',`Use ${next} theme`);button.title=`Use ${next} theme`;button.querySelector('use').setAttribute('href',theme==='dark'?'#i-sun':'#i-moon');
  try{window.__TAURI__.window.getCurrentWindow().setTheme?.(theme)?.catch?.(()=>{});}catch{/* older runtime: title bar keeps the system theme */}
}
function setView(view){
  $('comparison').dataset.view=view;for(const b of $('view-switch').children)b.setAttribute('aria-pressed',String(b.dataset.view===view));
  try{localStorage.setItem('view',view);}catch{/* storage unavailable */}
}
function setFocus(on){
  document.body.classList.toggle('focus',on);const b=$('preview-open');b.setAttribute('aria-pressed',String(on));
  const label=on?'Show batch settings':'Enlarge preview (hide batch settings)';b.setAttribute('aria-label',label);b.title=label;b.querySelector('use').setAttribute('href',on?'#i-collapse':'#i-expand');
}
function setupLayout(){
  applyTheme(document.documentElement.dataset.theme==='light'?'light':'dark');
  $('theme-toggle').onclick=()=>applyTheme(document.documentElement.dataset.theme==='dark'?'light':'dark');
  let view='split';try{view=localStorage.getItem('view')||'split';}catch{/* storage unavailable */}
  setView(['split','after','before'].includes(view)?view:'split');
  for(const b of $('view-switch').children)b.onclick=()=>setView(b.dataset.view);
  $('preview-open').onclick=()=>setFocus(!document.body.classList.contains('focus'));
  // Below 940 px the layout stacks and the preview already spans the window, so Enlarge has
  // nothing to do: hide it and leave enlarged mode (must match the CSS breakpoint).
  const stacked=matchMedia('(max-width:940px)');
  const syncStacked=()=>{if(stacked.matches&&document.body.classList.contains('focus'))setFocus(false);$('preview-open').hidden=stacked.matches;};
  stacked.addEventListener('change',syncStacked);syncStacked();
  $('filmstrip').addEventListener('wheel',event=>{if(Math.abs(event.deltaY)>Math.abs(event.deltaX)){$('filmstrip').scrollLeft+=event.deltaY;event.preventDefault();}},{passive:false});
}
function presetIndex(){return preset?builtins.findIndex(b=>b.file===preset.file&&b.name===preset.name):-1;}
function showPreset(){
  const select=$('preset-select'),index=presetIndex();
  select.replaceChildren(new Option('None','none'),...builtins.map((b,i)=>new Option(b.name,'b'+i)));
  if(preset&&index<0)select.add(new Option('From file: '+preset.name,'file'));
  select.value=!preset?'none':index<0?'file':'b'+index;$('preset-details').hidden=!preset;
  $('preset-note').textContent=preset?`${preset.name} (${preset.file}). Applied after Quick Fix; looks similar to Lightroom, not identical.`:'None. Choose a built-in preset, or From file… for a Lightroom .xmp or .lrtemplate develop preset.';
  const items=preset?[...preset.applied.map(t=>['Applied: ',t]),...preset.ignored.map(t=>['Not applied: ',t])]:[];
  $('preset-list').replaceChildren(...items.map(([state,t])=>{const li=document.createElement('li');const sr=document.createElement('span');sr.className='sr-only';sr.textContent=state;li.append(sr,t);if(state!=='Applied: ')li.className='off';return li;}));
}
function restoreRecipe(r){
  recipe=r;sources=r.sources;$('source-mode').value='files';$('source').value=`${sources.length} saved source paths`;
  for(const id of ['destination','suffix','format','quality','workers'])$(id).value=r[id];
  if(![...$('memory').options].some(o=>Number(o.value)===r.memory_mib))$('memory').add(new Option(`${r.memory_mib} MiB`,r.memory_mib));
  $('memory').value=r.memory_mib;$('recursive').checked=r.recursive;$('retain-capture').checked=r.retain_capture;
  $('resize').checked=r.size.resize;$('fit').value=r.size.constrain?(r.size.fit??'fit'):'stretch';$('match-orientation').checked=r.size.match_orientation??false;for(const id of ['width','height','unit'])$(id).value=r.size[id]??'';
  oldUnit=r.size.unit;$('density-mode').value=r.size.ppi===null?'keep':'set';$('ppi').value=r.size.ppi??300;
  for(const k of ['levels','contrast','color','sharpen'])$(k).checked=r.quick[k];
  preset=r.preset??null;showPreset();adjust=structuredClone(r.adjust??adjustDefaults);refreshAdjust();
  for(const[k]of manualControls){$('manual-'+k).value=r.manual[k];$('manual-'+k).nextElementSibling.value=r.manual[k];}
  selected=null;selectedInfo=null;page=0;++scanGeneration;++revision;markModified();
}
init().catch(showError);
