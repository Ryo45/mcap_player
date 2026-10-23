from pathlib import Path
import sqlite3, hashlib, json, struct, statistics
root=Path('/home/ryo/development/mcap_player')
p=root/'target/real-recording-check/input/source.db3'
c=sqlite3.connect('file:'+str(p)+'?immutable=1',uri=True)
topics=c.execute('select * from topics').fetchall()
rows=c.execute('select topic_id,timestamp,data from messages order by timestamp,id').fetchall()
facts={'source':'racer/tools/rosbag/rosbag2_2021_09_25-16_16_42/rosbag2_2021_09_25-16_16_42_0.db3','source_sha256':hashlib.file_digest(p.open('rb'),'sha256').hexdigest(),'messages':len(rows),'start_ns':rows[0][1],'end_ns':rows[-1][1],'topics':topics}
images=[]
for tid,t,data in rows:
 if tid!=6:continue
 # CDR encapsulation, Header(sec,nsec,string), Image dimensions and encoding.
 o=12;n=struct.unpack_from('<I',data,o)[0];o+=4+n;o=(o+3)&~3
 height,width,n=struct.unpack_from('<III',data,o);o+=12
 encoding=data[o:o+n-1].decode();o+=n;endian=data[o];o+=1;o=(o+3)&~3
 step,size=struct.unpack_from('<II',data,o);o+=8
 assert len(data)==o+size and size==height*step
 images.append({'time_ns':t,'width':width,'height':height,'encoding':encoding,'step':step,'data_sha256':hashlib.sha256(data).hexdigest()})
 from PIL import Image
 output=root/'target/real-recording-check/images';output.mkdir(exist_ok=True)
 Image.frombytes('RGB',(width,height),data[o:]).save(output/f'{len(images)-1:03d}.png')
facts['images']=images
facts['image_interval_ms']=statistics.median((b['time_ns']-a['time_ns'])/1e6 for a,b in zip(images,images[1:]))
(root/'target/real-recording-check/source.json').write_text(json.dumps(facts,indent=2)+'\n')
print({k:v for k,v in facts.items() if k not in ['images','topics']})
