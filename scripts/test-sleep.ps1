[CmdletBinding()]
param([ValidateRange(20,60)][int]$WakeAfterSeconds=30,[Parameter(Mandatory)][string]$ResultPath)
$ErrorActionPreference='Stop'
# Opt-in physical sleep test. A resume-capable wake timer must succeed first.
Add-Type @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
public static class ConnectySleepTest {
 [DllImport("kernel32.dll", SetLastError=true)] static extern IntPtr CreateWaitableTimer(IntPtr a,bool manual,string name);
 [DllImport("kernel32.dll", SetLastError=true)] static extern bool SetWaitableTimer(IntPtr timer,ref long due,int period,IntPtr routine,IntPtr arg,bool resume);
 [DllImport("kernel32.dll")] static extern void SetLastError(uint error);
 [DllImport("kernel32.dll")] static extern uint WaitForSingleObject(IntPtr handle,uint timeout);
 [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
 [DllImport("kernel32.dll")] static extern IntPtr GetCurrentProcess();
 [DllImport("advapi32.dll",SetLastError=true)] static extern bool OpenProcessToken(IntPtr process,uint rights,out IntPtr token);
 [DllImport("advapi32.dll",SetLastError=true,CharSet=CharSet.Unicode)] static extern bool LookupPrivilegeValue(string system,string name,out long luid);
 [StructLayout(LayoutKind.Sequential,Pack=4)] struct Privileges { public uint Count; public long Luid; public uint Attributes; }
 [DllImport("advapi32.dll",SetLastError=true)] static extern bool AdjustTokenPrivileges(IntPtr token,bool disable,ref Privileges value,uint length,IntPtr previous,IntPtr returned);
 [DllImport("powrprof.dll",SetLastError=true)] static extern bool SetSuspendState(bool hibernate,bool force,bool disableWake);
 public static void Sleep(int seconds) {
  IntPtr token;
  if(!OpenProcessToken(GetCurrentProcess(),0x28,out token)) throw new Win32Exception();
  try { long luid; if(!LookupPrivilegeValue(null,"SeShutdownPrivilege",out luid)) throw new Win32Exception();
   var p=new Privileges {Count=1,Luid=luid,Attributes=2}; SetLastError(0);
   if(!AdjustTokenPrivileges(token,false,ref p,0,IntPtr.Zero,IntPtr.Zero)||Marshal.GetLastWin32Error()!=0) throw new Win32Exception();
  } finally {CloseHandle(token);}
  IntPtr timer=CreateWaitableTimer(IntPtr.Zero,true,null);
  if(timer==IntPtr.Zero) throw new Win32Exception();
  try { long due=-(long)seconds*10000000; SetLastError(0);
   if(!SetWaitableTimer(timer,ref due,0,IntPtr.Zero,IntPtr.Zero,true)||Marshal.GetLastWin32Error()==50) throw new Win32Exception();
   Console.WriteLine("Automatic wake armed for "+seconds+" seconds; entering sleep.");
   if(!SetSuspendState(false,false,false)) throw new Win32Exception();
   if(WaitForSingleObject(timer,60000)!=0) throw new Exception("Wake timer did not signal");
  } finally {CloseHandle(timer);}
 }
}
'@
$began=Get-Date
[ConnectySleepTest]::Sleep($WakeAfterSeconds)
$finished=Get-Date
$events=Get-WinEvent -FilterHashtable @{LogName='System';StartTime=$began.AddSeconds(-1);Id=1,42,107} -ErrorAction SilentlyContinue |
 Where-Object {$_.ProviderName -in @('Microsoft-Windows-Kernel-Power','Microsoft-Windows-Power-Troubleshooter')} |
 Select-Object Id,ProviderName,TimeCreated,Message
@{started=$began.ToString('o');finished=$finished.ToString('o');elapsedSeconds=($finished-$began).TotalSeconds;events=@($events)} |
 ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $ResultPath
if(!($events | Where-Object {$_.Id -eq 42})){throw 'No physical sleep event 42 recorded; sleep is not proven'}
Write-Output "Physical sleep returned; evidence saved to $ResultPath"
