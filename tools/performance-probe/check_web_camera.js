(async()=>{
const errors=[];window.addEventListener('error',e=>errors.push(e.message));window.addEventListener('unhandledrejection',e=>errors.push(String(e.reason)));
const wait=async(f,msg)=>{for(let i=0;i<400;i++){if(f())return;await new Promise(r=>setTimeout(r,25));}throw Error(msg);};
const input=document.querySelector('#mcap-file');const files=new DataTransfer();files.items.add(new File([await(await fetch('gui-fixture.mcap')).arrayBuffer()],'fixture.mcap'));input.files=files.files;input.dispatchEvent(new Event('change',{bubbles:true}));
await wait(()=>document.querySelectorAll('.camera-card').length===7,'camera cards');
const line=document.querySelector('#timeline');line.value='0.4';line.dispatchEvent(new Event('input',{bubbles:true}));
await wait(()=>Number(line.value)>.39,'seek');
const c=document.querySelector('#camera');await wait(()=>c.width!==320 || c.getContext('2d').getImageData(10,10,1,1).data[3]!==0,'camera not painted');
await new Promise(r=>setTimeout(r,300));
document.querySelectorAll('.camera-card')[1].click();await new Promise(r=>setTimeout(r,300));
for(const v of ['0.8','0.2','0.6','0.1']){line.value=v;line.dispatchEvent(new Event('input',{bubbles:true}));await new Promise(r=>setTimeout(r,20));}
await wait(()=>Number(line.value)<.2,'final backward seek');await new Promise(r=>setTimeout(r,500));
const thumbs=[...document.querySelectorAll('.camera-card canvas')].map(c=>({width:c.width,height:c.height,alpha:c.getContext('2d').getImageData(Math.floor(c.width/2),Math.floor(c.height/2),1,1).data[3]}));
if(errors.length)throw Error(errors.join('\n'));if(thumbs.some(c=>c.alpha!==255))throw Error('thumbnail not painted');
return {viewport:[innerWidth,innerHeight],canvas:[c.width,c.height],thumbs,status:document.querySelector('#status').textContent,scene:document.querySelector('#bev-status').textContent,focus:document.querySelector('#camera-focus-label').textContent,timeline:line.value,errors};
})()
