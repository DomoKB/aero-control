# Run with Windows PowerShell 5.1. Exercises the actual embedded bridge with
# an in-memory SDK stand-in; no Intel DLLs, services or hardware are accessed.
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
public class FakeControl {
    public uint Id; public string Name; public string Units = "Watts";
    public bool Enabled = true, ReadOnly, RequiresReboot;
    public decimal ActiveValue;
    public List<decimal> SupportedValues = new List<decimal>{38m,45m,52m,58m,62m,107m,56.25m};
}
public class FakeResult {
    public bool Success;
    public static implicit operator bool(FakeResult r) { return r.Success; }
}
public class FakeTuning {
    public FakeControl Pl1 = new FakeControl { Id=48, Name="Turbo Boost Power Max", ActiveValue=52m };
    public FakeControl Pl2 = new FakeControl { Id=47, Name="Turbo Boost Short Power Max", ActiveValue=107m };
    public int Writes, Reads;
    public bool RejectSecond, IgnoreSecond, FailRollback;
    public FakeControl GetControl(uint id) { return id==48 ? Pl1 : Pl2; }
    public void RefreshActiveValuesFromHardwareAccess(bool onApply) { Reads++; }
    public FakeResult Tune(uint id, decimal value, bool reboot) {
        Writes++;
        if ((RejectSecond && Writes==2) || (FailRollback && Writes>=3)) return new FakeResult { Success=false };
        if (!(IgnoreSecond && Writes==2)) GetControl(id).ActiveValue=value;
        return new FakeResult { Success=true };
    }
}
'@

$source = [IO.File]::ReadAllText((Join-Path $PSScriptRoot '../src/cpu_power.ps1'))
$begin = $source.IndexOf('    $cpuName = ')
$end = $source.IndexOf('    $controls = Read-Controls', $begin)
if ($begin -lt 0 -or $end -le $begin) { throw 'Cannot locate SDK bootstrap for the test stand-in.' }
$source = $source.Substring(0, $begin) + '$script:Tuning = $Fake; $script:TuningType = $Fake.GetType();' + "`n" + $source.Substring($end)
$source = $source.Replace('[Console]::WriteLine((', 'Write-Output ((').Replace('exit 0', 'return')
$source = $source.Replace("[Console]::Error.WriteLine('Intel XTU: ' + `$_.Exception.GetBaseException().Message)", "throw `$_.Exception.GetBaseException().Message")
$bridge = [scriptblock]::Create($source)

function Run-Bridge($Fake, $First = $null, $Second = $null) {
    $RequestedPl1 = $First
    $RequestedPl2 = $Second
    & $bridge
}
function Assert($Condition, $Message) { if (-not $Condition) { throw $Message } }
function Expect-Failure($Fake, [string]$Pattern) {
    try { $null = Run-Bridge $Fake ([decimal]45) ([decimal]56.25) } catch {
        Assert ($_.Exception.Message -match $Pattern) "Unexpected error: $($_.Exception.Message)"
        return
    }
    throw 'Expected bridge failure.'
}

$fake = New-Object FakeTuning
$result = Run-Bridge $fake
Assert ($result -eq 'AERO_CPU_V1|52000|107000|127') 'Read protocol or availability mask mismatch.'
Assert ($fake.Writes -eq 0) 'A read must never tune a CPU control.'

$fake = New-Object FakeTuning
$result = Run-Bridge $fake ([decimal]45) ([decimal]56.25)
Assert ($result -eq 'AERO_CPU_V1|45000|56250|127') 'Fractional battery preset was lost.'
Assert ($fake.Writes -eq 2 -and $fake.Reads -eq 2) 'An apply must write both limits and refresh readback.'

$fake = New-Object FakeTuning
$fake.Pl2.ReadOnly = $true
Expect-Failure $fake 'unavailable or locked'
Assert ($fake.Writes -eq 0) 'A locked second control must prevent the first write.'

$fake = New-Object FakeTuning
$fake.RejectSecond = $true
Expect-Failure $fake 'Previous CPU limits were restored'
Assert ($fake.Pl1.ActiveValue -eq 52 -and $fake.Pl2.ActiveValue -eq 107) 'Rejected second write did not restore both limits.'

$fake = New-Object FakeTuning
$fake.IgnoreSecond = $true
Expect-Failure $fake 'Previous CPU limits were restored'
Assert ($fake.Pl1.ActiveValue -eq 52 -and $fake.Pl2.ActiveValue -eq 107) 'Silent no-op did not restore both limits.'

$fake = New-Object FakeTuning
$fake.RejectSecond = $true
$fake.FailRollback = $true
Expect-Failure $fake 'Recovery failed; CPU limits may be partially changed'

$fake = New-Object FakeTuning
$fake.Pl1.Units = 'Volts'
Expect-Failure $fake 'identities or units'
Assert ($fake.Writes -eq 0) 'Wrong control units must prevent all writes.'

Write-Output '7 CPU bridge scenarios passed (fake SDK only).'
