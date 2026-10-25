#!/usr/bin/env python3
"""Exercise competing editor connections against the real isolated D-Bus service."""
import json,os,time
import gi
gi.require_version('Gio','2.0')
from gi.repository import Gio,GLib

def connect():
 return Gio.DBusConnection.new_for_address_sync(os.environ['DBUS_SESSION_BUS_ADDRESS'],Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT|Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION,None,None)
def call(conn,method,sig='()',args=()):
 result=conn.call_sync('org.massiveeq.Service1','/org/massiveeq/Service1','org.massiveeq.Service1',method,GLib.Variant(sig,args),None,Gio.DBusCallFlags.NONE,5000,None).unpack()
 return json.loads(result[0]) if result and result[0].startswith('{') else result

a,b=connect(),connect()
p=call(a,'CreateProfile','(s)',('Ownership regression',)); id=p['id']
first='Filter 1: ON PK Fc 1000 Hz Gain -3 dB Q 1\n'
other='Filter 1: ON PK Fc 200 Hz Gain 9 dB Q 1\n'
def preview(conn,text):call(conn,'PreviewProfile','(ss)',(id,json.dumps({'text':text,'trim':0.0})))
def denied(method,fn):
 try:fn()
 except GLib.Error as e:
  assert 'AccessDenied' in str(e),str(e)
  print('PASS: competing editor cannot '+method,flush=True)
 else:raise AssertionError('competing editor was allowed to '+method)
preview(a,first)
before=call(a,'GetProfile','(s)',(id,))
denied('replace the live draft',lambda:preview(b,other))
denied('save over the active editor',lambda:call(b,'PutProfile','(sssd)',(id,'Stale editor',other,0.0)))
assert call(a,'GetProfile','(s)',(id,))==before
denied('end another editor preview',lambda:call(b,'EndPreview','(s)',(id,)))
preview(a,first)
call(a,'PutProfile','(sssd)',(id,'Ownership regression',first,0.0))
call(a,'EndPreview','(s)',(id,))
preview(b,other)
call(b,'PutProfile','(sssd)',(id,'Ownership regression',other,0.0))
print('PASS: owning editor can renew/save/end and ownership transfers after release',flush=True)
b.close_sync(None)
time.sleep(6)
preview(a,first)
call(a,'PutProfile','(sssd)',(id,'Ownership regression',first,0.0))
call(a,'EndPreview','(s)',(id,))
print('PASS: a crashed owner expires and does not lock out the next editor',flush=True)
call(a,'DeleteProfile','(s)',(id,))
a.close_sync(None)
