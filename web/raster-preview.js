// Header/container checks precede the native decoder. These are resource
// admission limits, not a replacement for the browser's image codec sandbox.
export const MAX_IMAGE_BYTES=2*1024*1024;
export function rasterInfo(buffer,path){
 const b=new Uint8Array(buffer),v=new DataView(buffer);
 const fail=()=>{throw Error('Unsupported or malformed bounded PNG/JPEG.');};
 const dimensions=(width,height,mime)=>{
  if(!width||!height||width>4096||height>4096||width*height>4194304)throw Error('Image exceeds 4096 per side or 4 megapixels.');
  return {width,height,mime};
 };
 if(b.length>MAX_IMAGE_BYTES)throw Error('Image exceeds 2 MiB.');
 if(/\.png$/i.test(path)){
  if(b.length<33||![137,80,78,71,13,10,26,10].every((x,i)=>b[i]===x))return fail();
  let at=8,info=null,idat=false,endedData=false;
  while(at+12<=b.length){
   const n=v.getUint32(at),end=at+12+n;
   if(end>b.length)return fail();
   const type=String.fromCharCode(...b.subarray(at+4,at+8));
   if(!/^[A-Za-z]{4}$/.test(type)||['acTL','fcTL','fdAT','iCCP','zTXt','iTXt'].includes(type))return fail();
   if(type==='IHDR'){
    if(at!==8||n!==13||info)return fail();
    info=dimensions(v.getUint32(at+8),v.getUint32(at+12),'image/png');
    const depth=b[at+16],color=b[at+17];
    if(!({0:[1,2,4,8,16],2:[8,16],3:[1,2,4,8],4:[8,16],6:[8,16]})[color]?.includes(depth)||b[at+18]!==0||b[at+19]!==0||b[at+20]>1)return fail();
   }else if(!info)return fail();
   else if(type==='IDAT'){if(endedData)return fail();idat=true;}
   else if(type==='IEND'){if(n||!idat||end!==b.length)return fail();return info;}
   else {
    if(idat)endedData=true;
    // No compressed metadata or unknown ancillary extensions reach native code.
    const lengths={sRGB:1,gAMA:4,cHRM:32,pHYs:9};
    if(type==='PLTE'){if(!n||n>768||n%3)return fail();}
    else if(type==='tRNS'){if(n>256)return fail();}
    else if(lengths[type]!==n)return fail();
   }
   at=end;
  }
  return fail();
 }
 if(!/\.jpe?g$/i.test(path)||b[0]!==255||b[1]!==216)return fail();
 // Conservative single-scan baseline JPEG only. Reject progressive,
 // multi-frame, DNL and trailing concatenated images before decoding.
 let at=2,info=null,scan=false,componentIds=[];
 while(at<b.length){
  if(b[at++]!==255)return fail();
  while(b[at]===255)at++;
  const marker=b[at++];
  if(marker===217){if(!scan||at!==b.length)return fail();return info;}
  if(scan)return fail();
  if(![192,196,218,219,221,254].includes(marker)&&!(marker===224||marker===238))return fail();
  if(at+2>b.length)return fail();
  const size=v.getUint16(at),end=at+size;if(size<2||end>b.length)return fail();
  if(marker===192){
   if(info||size<8||b[at+2]!==8)return fail();
   const components=b[at+7];if(![1,3].includes(components)||size!==8+components*3)return fail();
   let blocks=0;
   for(let i=0;i<components;i++){
    const offset=at+8+i*3,id=b[offset],sampling=b[offset+1],h=sampling>>4,w=sampling&15;
    if(componentIds.includes(id)||!h||h>4||!w||w>4||b[offset+2]>3)return fail();
    componentIds.push(id);blocks+=h*w;
   }
   if(blocks>10)return fail();
   info=dimensions(v.getUint16(at+5),v.getUint16(at+3),'image/jpeg');
  }
  if(marker===218){
   if(!info||size<6)return fail();
   const components=b[at+2];if(![1,3].includes(components)||size!==6+2*components||b[end-3]!==0||b[end-2]!==63||b[end-1]!==0)return fail();
   if(components!==componentIds.length)return fail();
   const selected=[];
   for(let i=0;i<components;i++){
    const id=b[at+3+i*2],table=b[at+4+i*2];
    if(!componentIds.includes(id)||selected.includes(id)||(table>>4)>3||(table&15)>3)return fail();
    selected.push(id);
   }
   scan=true;at=end;
   while(at<b.length){
    if(b[at]!==255){at++;continue;}
    if(b[at+1]===0||(b[at+1]>=208&&b[at+1]<=215)){at+=2;continue;}
    break;
   }
  }else at=end;
 }
 return fail();
}
export async function decodeRaster(buffer,info){
 if(typeof globalThis.createImageBitmap!=='function')throw Error('Bounded image decoding is unavailable in this WebView.');
 return globalThis.createImageBitmap(new Blob([buffer],{type:info.mime}),{imageOrientation:'none'});
}
