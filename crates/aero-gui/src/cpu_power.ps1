# Invoked only by the serialized CPU-power worker. No vendor files are copied.
# $RequestedPl1/$RequestedPl2 are fixed numeric literals supplied by Rust, in W.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)

function Invoke-Sdk($Type, $Object, $Name, [object[]]$Arguments) {
    $methods = @($Type.GetMethods() | Where-Object { $_.Name -eq $Name -and $_.GetParameters().Count -eq $Arguments.Count })
    if ($Name -eq 'Tune') {
        $methods = @($methods | Where-Object { $_.GetParameters()[1].ParameterType -eq [decimal] })
    }
    if ($methods.Count -ne 1) { throw "Unsupported Intel SDK method: $Name" }
    return $methods[0].Invoke($Object, $Arguments)
}

function Read-Controls {
    $null = Invoke-Sdk $script:TuningType $script:Tuning 'RefreshActiveValuesFromHardwareAccess' @($false)
    $first = Invoke-Sdk $script:TuningType $script:Tuning 'GetControl' @([uint32]48)
    $second = Invoke-Sdk $script:TuningType $script:Tuning 'GetControl' @([uint32]47)
    if ($null -eq $first -or $null -eq $second -or $first.Id -ne 48 -or $second.Id -ne 47) {
        throw 'Intel XTU did not expose both CPU power limits.'
    }
    if ($first.Name -notmatch 'Turbo.*Power.*Max' -or $first.Name -match 'Short' -or
        $second.Name -notmatch 'Turbo.*Short.*Power.*Max' -or
        $first.Units -notmatch '^(W|Watt|Watts)$' -or $second.Units -notmatch '^(W|Watt|Watts)$') {
        throw 'Intel XTU control identities or units do not match the verified CPU power controls.'
    }
    return @($first, $second)
}

function Can-Set($Control, [decimal]$Value) {
    return $Control.Enabled -and -not $Control.ReadOnly -and -not $Control.RequiresReboot -and
        $null -ne $Control.SupportedValues -and
        $Control.SupportedValues.Contains($Value)
}

function Tune-Checked([uint32]$Id, [decimal]$Value) {
    $result = Invoke-Sdk $script:TuningType $script:Tuning 'Tune' @($Id, $Value, $false)
    if ($result -is [bool]) {
        if (-not $result) { throw "Intel XTU rejected control $Id." }
    } else {
        $conversion = $result.GetType().GetMethod('op_Implicit', [type[]]@($result.GetType()))
        if ($null -eq $conversion -or -not $conversion.Invoke($null, @($result))) {
            throw "Intel XTU rejected control ${Id}: $result"
        }
    }
    Start-Sleep -Milliseconds 500
}

try {
    $cpuName = (Get-ItemProperty -LiteralPath 'HKLM:\HARDWARE\DESCRIPTION\System\CentralProcessor\0' -Name ProcessorNameString).ProcessorNameString
    if ($cpuName -notmatch 'Intel.*i7-10875H') { throw 'These CPU power presets are verified only for the Intel Core i7-10875H.' }
    $base = [Environment]::GetFolderPath([Environment+SpecialFolder]::ProgramFiles)
    $sdkPath = Join-Path $base 'Intel\Intel(R) Extreme Tuning Utility\Client\IntelOverclockingSDK.dll'
    if (-not (Test-Path -LiteralPath $sdkPath -PathType Leaf)) {
        throw 'A compatible Intel XTU installation is required for CPU power control.'
    }
    $signature = Get-AuthenticodeSignature -LiteralPath $sdkPath
    if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch '(Intel|Microsoft)') {
        throw 'The installed Intel XTU SDK signature could not be verified.'
    }
    $sdk = [Reflection.Assembly]::LoadFrom($sdkPath)
    $serviceType = $sdk.GetType('Intel.Overclocking.SDK.ServiceInfo.IServiceInfoLibrary', $true)
    $service = $sdk.GetType('Intel.Overclocking.SDK.ServiceInfo.ServiceInfoLibrary', $true).GetProperty('Instance').GetValue($null)
    $null = Invoke-Sdk $serviceType $service 'Initialize' @()
    if ($null -ne $serviceType.GetMethod('GetSystemCompatibilityFlags')) {
        $flags = Invoke-Sdk $serviceType $service 'GetSystemCompatibilityFlags' @()
        if ([uint64]$flags -ne 0) {
            throw "Intel XTU CPU tuning is unavailable (compatibility: $flags). Check the installed SDK/service and Intel XTU diagnostics."
        }
    }
    $issues = @()
    if ($null -ne $serviceType.GetMethod('ListPlatformCompatibilityIssues')) {
        $issues = @(Invoke-Sdk $serviceType $service 'ListPlatformCompatibilityIssues' @())
    }
    if ($issues.Count -gt 0) {
        throw "Intel XTU has blocked CPU tuning on this system: $($issues -join '; ')"
    }
    $script:TuningType = $sdk.GetType('Intel.Overclocking.SDK.Tuning.ITuningLibrary', $true)
    $script:Tuning = $sdk.GetType('Intel.Overclocking.SDK.Tuning.TuningLibrary', $true).GetProperty('Instance').GetValue($null)
    $null = Invoke-Sdk $script:TuningType $script:Tuning 'Initialize' @()
    $availableControls = @(Invoke-Sdk $script:TuningType $script:Tuning 'GetAvailableControls' @())
    if ($availableControls.Count -eq 0) {
        throw 'Intel XTU exposes no CPU tuning controls. A compatible SDK/service and processor support are required.'
    }
    $controls = Read-Controls
    if ($null -ne $RequestedPl1) {
        if (-not (Can-Set $controls[0] $RequestedPl1) -or -not (Can-Set $controls[1] $RequestedPl2)) {
            throw 'The requested CPU preset is unavailable or locked in Intel XTU. No limits were changed.'
        }
        $old1 = [decimal]$controls[0].ActiveValue
        $old2 = [decimal]$controls[1].ActiveValue
        if (-not (Can-Set $controls[0] $old1) -or -not (Can-Set $controls[1] $old2)) {
            throw 'Current CPU limits cannot be restored through Intel XTU. No limits were changed.'
        }
        try {
            $null = Tune-Checked 48 $RequestedPl1
            $null = Tune-Checked 47 $RequestedPl2
            $controls = Read-Controls
            if ($controls[0].ActiveValue -ne $RequestedPl1 -or $controls[1].ActiveValue -ne $RequestedPl2) {
                throw 'CPU power readback did not match the requested limits.'
            }
        } catch {
            $failure = $_.Exception.GetBaseException().Message
            $rollbackErrors = @()
            foreach ($entry in @(@([uint32]48, $old1), @([uint32]47, $old2))) {
                try { $null = Tune-Checked $entry[0] $entry[1] } catch { $rollbackErrors += $_.Exception.GetBaseException().Message }
            }
            try {
                $restored = Read-Controls
                if ($restored[0].ActiveValue -ne $old1 -or $restored[1].ActiveValue -ne $old2) { throw 'CPU rollback readback mismatch.' }
            } catch { $rollbackErrors += $_.Exception.GetBaseException().Message }
            if ($rollbackErrors.Count -gt 0) { throw "$failure Recovery failed; CPU limits may be partially changed: $($rollbackErrors -join '; ')" }
            throw "$failure Previous CPU limits were restored."
        }
    }
    $mask = 0
    $presets = @(@(38,107), @(45,107), @(52,107), @(58,107), @(62,107), @(52,107), @(45,56.25))
    for ($i = 0; $i -lt $presets.Count; $i++) {
        if ((Can-Set $controls[0] ([decimal]$presets[$i][0])) -and (Can-Set $controls[1] ([decimal]$presets[$i][1]))) { $mask = $mask -bor (1 -shl $i) }
    }
    $pl1 = [decimal]$controls[0].ActiveValue * 1000
    $pl2 = [decimal]$controls[1].ActiveValue * 1000
    if ($pl1 -ne [math]::Truncate($pl1) -or $pl2 -ne [math]::Truncate($pl2)) { throw 'Unsupported CPU power precision.' }
    [Console]::WriteLine(('AERO_CPU_V1|{0}|{1}|{2}' -f [long]$pl1, [long]$pl2, $mask))
    exit 0
} catch {
    [Console]::Error.WriteLine('Intel XTU: ' + $_.Exception.GetBaseException().Message)
    exit 1
}
