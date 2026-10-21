// Some browsers apply EXIF even with imageOrientation:none. Optical coordinates
// must use the encoded scan direction. Blob parts omit EXIF without copying the scan.
function opticalJpegParts(buffer) {
  const bytes = new Uint8Array(buffer);
  if (bytes[0] !== 0xff || bytes[1] !== 0xd8) throw new Error('invalid JPEG SOI');
  const parts = [];
  let offset = 2, retained = 0;
  for (let count=0;count<4096;count++) {
    const start = offset;
    if (bytes[offset] !== 0xff) throw new Error('invalid JPEG marker');
    while (bytes[offset] === 0xff) offset++;
    if (offset >= bytes.length) throw new Error('truncated JPEG marker');
    const marker = bytes[offset++];
    if (marker === 0xda || marker === 0xd9) {
      parts.push(bytes.subarray(retained)); return parts;
    }
    if (marker === 0x01 || (marker>=0xd0 && marker<=0xd7)) continue;
    if (marker === 0 || marker === 0xd8 || offset+2 > bytes.length) throw new Error('invalid JPEG header');
    const length = bytes[offset]*256 + bytes[offset+1];
    if (length<2 || offset+length>bytes.length) throw new Error('invalid JPEG segment length');
    if (marker===0xe1 && length>=8 && bytes[offset+2]===69 && bytes[offset+3]===120
        && bytes[offset+4]===105 && bytes[offset+5]===102 && bytes[offset+6]===0 && bytes[offset+7]===0) {
      parts.push(bytes.subarray(retained,start)); retained=offset+length;
    }
    offset+=length;
  }
  throw new Error('JPEG exceeds 4096 header markers');
}

self.onmessage = async ({data}) => {
  let bitmap;
  try {
    if (!(data.bytes instanceof ArrayBuffer) || data.bytes.byteLength > 32 * 1024 * 1024
        || !Number.isInteger(data.width) || !Number.isInteger(data.height)
        || data.width <= 0 || data.height <= 0
        || data.width * data.height > 32 * 1024 * 1024) {
      throw new Error('invalid camera bitmap request');
    }
    let input;
    if (data.format === 'jpeg' || data.format === 'png') {
      input = new Blob(data.format === 'jpeg' ? opticalJpegParts(data.bytes) : [data.bytes],
        {type: data.format === 'jpeg' ? 'image/jpeg' : 'image/png'});
    } else {
      const channels = {rgb8:3, bgr8:3, rgba8:4, bgra8:4, mono8:1}[data.format];
      if (!channels || !Number.isInteger(data.sourceWidth) || !Number.isInteger(data.sourceHeight)
          || data.sourceWidth <= 0 || data.sourceHeight <= 0
          || data.sourceWidth * data.sourceHeight > 32*1024*1024
          || !Number.isInteger(data.step) || data.step < data.sourceWidth*channels
          || data.step * data.sourceHeight !== data.bytes.byteLength
          || data.width > data.sourceWidth || data.height > data.sourceHeight) {
        throw new Error('invalid raw image layout');
      }
      const source = new Uint8Array(data.bytes);
      const rgba = new Uint8ClampedArray(data.width * data.height * 4);
      for (let y=0; y<data.height; y++) {
        const row = Math.floor(y*data.sourceHeight/data.height)*data.step;
        for (let x=0; x<data.width; x++) {
          const i = row + Math.floor(x*data.sourceWidth/data.width)*channels;
          const o = (y*data.width+x)*4;
          const mono = data.format === 'mono8';
          const bgr = data.format === 'bgr8' || data.format === 'bgra8';
          rgba[o] = source[i+(bgr ? 2 : 0)];
          rgba[o+1] = source[i+(mono ? 0 : 1)];
          rgba[o+2] = source[i+(mono ? 0 : bgr ? 0 : 2)];
          rgba[o+3] = channels === 4 ? source[i+3] : 255;
        }
      }
      input = new ImageData(rgba, data.width, data.height);
    }
    data.bytes = null;
    bitmap = await createImageBitmap(input, {
      resizeWidth: data.width, resizeHeight: data.height, resizeQuality: 'low',
      imageOrientation: 'none', colorSpaceConversion: 'none', premultiplyAlpha: 'none',
    });
    if (bitmap.width !== data.width || bitmap.height !== data.height) {
      throw new Error('camera bitmap dimensions differ from request');
    }
    self.postMessage({bitmap}, [bitmap]);
  } catch (error) {
    bitmap?.close();
    self.postMessage({error: String(error)});
  }
};
