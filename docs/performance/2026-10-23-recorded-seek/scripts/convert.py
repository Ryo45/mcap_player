import sqlite3,json,hashlib
from pathlib import Path
import rosbag2_py
base=Path('target/real-recording-check')
c=sqlite3.connect('file:'+str(base/'input/source.db3')+'?immutable=1',uri=True)
w=rosbag2_py.SequentialWriter()
w.open(rosbag2_py.StorageOptions(uri=str(base/'mcap-verified'),storage_id='mcap'),rosbag2_py.ConverterOptions('',''))
topics={}
for tid,name,kind,encoding in c.execute('select * from topics'):
 topics[tid]=name
 w.create_topic(rosbag2_py.TopicMetadata(id=tid,name=name,type=kind,serialization_format=encoding))
for tid,t,data in c.execute('select topic_id,timestamp,data from messages order by timestamp,id'):
 w.write(topics[tid],data,t)
del w
r=rosbag2_py.SequentialReader()
r.open(rosbag2_py.StorageOptions(uri=str(base/'mcap-verified'),storage_id='mcap'),rosbag2_py.ConverterOptions('',''))
expected=[(topics[tid],bytes(data),t)for tid,t,data in c.execute('select topic_id,timestamp,data from messages order by timestamp,id')]
i=0
while r.has_next():
 assert r.read_next()==expected[i],i
 i+=1
assert i==len(expected)
print('VERIFIED',i,'identical topics, log times and CDR payloads')
