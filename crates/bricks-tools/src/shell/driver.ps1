# Bricks PowerShell driver. Runs at the top level of one long-lived pwsh /
# powershell process; each request's script block is dot-sourced, so
# variables, functions, aliases, $env: and Set-Location persist.
#
# Control channel: a loopback TCP connection to Bricks (port and one-time
# token in the environment), JSON lines both ways. The process's stdin is
# empty and is never used for control. Every output item, error record and
# the final status travel on that channel, tagged with the request id.
#
# Names starting with `__bricks` are reserved.
$ErrorActionPreference = 'Continue'
$__bricksClient = [System.Net.Sockets.TcpClient]::new('127.0.0.1', [int]$env:BRICKS_PS_PORT)
$__bricksStream = $__bricksClient.GetStream()
$__bricksUtf8 = [System.Text.UTF8Encoding]::new($false)
$__bricksReader = [System.IO.StreamReader]::new($__bricksStream, $__bricksUtf8)
$__bricksWriter = [System.IO.StreamWriter]::new($__bricksStream, $__bricksUtf8)
$__bricksWriter.AutoFlush = $true
function __bricksSend($obj) { $__bricksWriter.WriteLine(($obj | ConvertTo-Json -Compress -Depth 4)) }
__bricksSend @{ kind = 'ready'; token = $env:BRICKS_PS_TOKEN; pid = $PID; version = "$($PSVersionTable.PSVersion)" }
Remove-Item Env:BRICKS_PS_TOKEN, Env:BRICKS_PS_PORT -ErrorAction SilentlyContinue
while ($true) {
    $__bricksLine = $__bricksReader.ReadLine()
    if ($null -eq $__bricksLine) { break }
    $__bricksReq = $__bricksLine | ConvertFrom-Json
    $__bricksCode = [System.Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($__bricksReq.script))
    # A command that runs no native program must not report an old code.
    $global:LASTEXITCODE = $null
    $__bricksCmdletErrors = 0
    $__bricksOk = $true
    try {
        $__bricksBlock = [ScriptBlock]::Create($__bricksCode)
        . $__bricksBlock 2>&1 | ForEach-Object {
            if ($_ -is [System.Management.Automation.ErrorRecord]) {
                $__bricksNative = $_.FullyQualifiedErrorId -like 'NativeCommandError*'
                if (-not $__bricksNative) { $__bricksCmdletErrors++ }
                $__bricksText = if ($__bricksNative) { "$($_.Exception.Message)`n" } else { $_ | Out-String }
                __bricksSend @{ kind = 'err'; id = $__bricksReq.id; native = $__bricksNative; text = $__bricksText }
            } else {
                __bricksSend @{ kind = 'out'; id = $__bricksReq.id; text = ($_ | Out-String -Width 4096) }
            }
        }
        $__bricksOk = $?
    } catch {
        $__bricksCmdletErrors++
        $__bricksOk = $false
        __bricksSend @{ kind = 'err'; id = $__bricksReq.id; native = $false; text = ($_ | Out-String) }
    }
    __bricksSend @{
        kind = 'end'; id = $__bricksReq.id; ok = $__bricksOk; cmdlet_errors = $__bricksCmdletErrors
        native_exit = $global:LASTEXITCODE; cwd = (Get-Location).Path
    }
}
