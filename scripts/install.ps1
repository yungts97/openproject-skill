<#
.SYNOPSIS
Installs or upgrades the OpenProject CLI.

.PARAMETER Version
Release version to install, with or without a leading "v". Defaults to the latest GitHub release.

.PARAMETER Destination
Installation directory. Defaults to OPENPROJECT_INSTALL_DIR when set, then the user's local application data directory.

.EXAMPLE
./install.ps1

.EXAMPLE
./install.ps1 -Version 0.1.2 -Destination C:\Tools
#>
[CmdletBinding()]
param(
  [Parameter(Position = 0)]
  [ValidateNotNullOrEmpty()]
  [string]$Version = "latest",

  [Parameter(Position = 1)]
  [ValidateNotNullOrEmpty()]
  [string]$Destination = $(
    if ($env:OPENPROJECT_INSTALL_DIR) {
      $env:OPENPROJECT_INSTALL_DIR
    } else {
      Join-Path ([Environment]::GetFolderPath("LocalApplicationData")) "openproject\bin"
    }
  )
)

& {
$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Write-Step {
  param([int]$Number, [string]$Message)
  Write-Host "[$Number/4] $Message"
}

$Repository = if ($env:OPENPROJECT_RELEASE_REPOSITORY) { $env:OPENPROJECT_RELEASE_REPOSITORY.Trim("/") } else { "yungts97/openproject-skill" }
$RequestedVersion = $Version.Trim()
$Version = $RequestedVersion
if ($RequestedVersion.StartsWith("v", [StringComparison]::OrdinalIgnoreCase)) {
  $Version = $RequestedVersion.Substring(1)
}
if (-not $Version) {
  throw "The release version cannot be empty."
}

$Architecture = [System.Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture.ToString()
$Target = switch ($Architecture) {
  "X64" { "x86_64-pc-windows-msvc" }
  "Arm64" { "aarch64-pc-windows-msvc" }
  default { throw "Processor architecture '$Architecture' is not supported. Supported architectures: X64 and Arm64." }
}

$Destination = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Destination)
$Archive = "openproject-$Target.zip"
$Checksums = "SHA256SUMS"
$Executable = Join-Path $Destination "openproject.exe"
$Action = if (Test-Path -LiteralPath $Executable) { "Upgraded" } else { "Installed" }
$Temporary = $null
$Staged = $null

Write-Host "OpenProject CLI installer"
Write-Host ""
Write-Host "  Version:     $Version"
Write-Host "  Target:      $Target"
Write-Host "  Destination: $Executable"
Write-Host ""

try {
  Write-Step 1 "Checking system requirements"
  if ($env:OPENPROJECT_GITLAB_PROJECT) {
    if ($Version -eq "latest") {
      throw "A specific release version is required with OPENPROJECT_GITLAB_PROJECT."
    }
    if (-not (Get-Command glab -ErrorAction SilentlyContinue)) {
      throw "Required command 'glab' was not found on PATH. Install it and authenticate before using OPENPROJECT_GITLAB_PROJECT."
    }
  }

  $Temporary = Join-Path ([IO.Path]::GetTempPath()) ("openproject-" + [guid]::NewGuid())
  New-Item -ItemType Directory -Force -Path $Temporary | Out-Null

  Write-Step 2 "Downloading $Archive"
  if ($env:OPENPROJECT_GITLAB_PROJECT) {
    $GlabArguments = @("release", "download", $RequestedVersion)
    if ($env:OPENPROJECT_GITLAB_HOST) {
      $GlabArguments += @("--hostname", $env:OPENPROJECT_GITLAB_HOST)
    }
    $GlabArguments += @("--repo", $env:OPENPROJECT_GITLAB_PROJECT, "--pattern", $Archive, "--pattern", $Checksums, "--dir", $Temporary)
    & glab @GlabArguments
    if ($LASTEXITCODE -ne 0) {
      throw "Could not download release $RequestedVersion from GitLab. Check the version and your glab authentication."
    }
  } else {
    $Base = if ($Version -eq "latest") {
      "https://github.com/$Repository/releases/latest/download"
    } else {
      "https://github.com/$Repository/releases/download/v$Version"
    }
    try {
      Invoke-WebRequest "$Base/$Archive" -OutFile (Join-Path $Temporary $Archive)
    } catch {
      throw "Could not download $Archive. Check the release version and your network connection. $($_.Exception.Message)"
    }
    try {
      Invoke-WebRequest "$Base/$Checksums" -OutFile (Join-Path $Temporary $Checksums)
    } catch {
      throw "Could not download $Checksums. The release may be incomplete. $($_.Exception.Message)"
    }
  }

  Write-Step 3 "Verifying the SHA-256 checksum"
  $ChecksumPattern = "^[a-fA-F0-9]{64}\s+\*?$([regex]::Escape($Archive))$"
  $ChecksumLine = Get-Content (Join-Path $Temporary $Checksums) |
    Where-Object { $_ -match $ChecksumPattern } |
    Select-Object -First 1
  if (-not $ChecksumLine) {
    throw "No checksum was published for $Archive."
  }
  $Expected = ($ChecksumLine -split "\s+", 2)[0]
  $Actual = (Get-FileHash (Join-Path $Temporary $Archive) -Algorithm SHA256).Hash
  if ($Actual -ne $Expected) {
    throw "Checksum verification failed. The downloaded archive may be damaged or unsafe."
  }

  Write-Step 4 "$Action OpenProject CLI"
  try {
    New-Item -ItemType Directory -Force -Path $Destination | Out-Null
  } catch {
    throw "Could not create $Destination. Choose a writable directory with -Destination or OPENPROJECT_INSTALL_DIR. $($_.Exception.Message)"
  }

  $ExtractedDirectory = Join-Path $Temporary "extracted"
  Expand-Archive (Join-Path $Temporary $Archive) -DestinationPath $ExtractedDirectory -Force
  $ExtractedExecutable = Join-Path $ExtractedDirectory "openproject.exe"
  if (-not (Test-Path -LiteralPath $ExtractedExecutable -PathType Leaf)) {
    throw "The release archive does not contain openproject.exe."
  }

  $Staged = Join-Path $Destination ".openproject.new.$PID.exe"
  Copy-Item -LiteralPath $ExtractedExecutable -Destination $Staged -Force
  try {
    Move-Item -LiteralPath $Staged -Destination $Executable -Force
  } catch {
    throw "Could not replace $Executable. Make sure it is not in use and try again. $($_.Exception.Message)"
  }
  $Staged = $null

  Write-Host ""
  Write-Host "Success: $Action $Executable"
  $PathEntries = @($env:PATH -split [IO.Path]::PathSeparator | ForEach-Object { $_.TrimEnd("\") })
  if ($PathEntries -notcontains $Destination.TrimEnd("\")) {
    Write-Host ""
    Write-Host "Note: $Destination is not on PATH. Add it to your user PATH, then open a new terminal."
  }
  Write-Host ""
  Write-Host "Verify the installation:"
  Write-Host "  & '$Executable' --version"
} catch {
  throw "OpenProject installation failed: $($_.Exception.Message)"
} finally {
  if ($Staged -and (Test-Path -LiteralPath $Staged)) {
    Remove-Item -LiteralPath $Staged -Force -ErrorAction SilentlyContinue
  }
  if ($Temporary -and (Test-Path -LiteralPath $Temporary)) {
    Remove-Item -LiteralPath $Temporary -Recurse -Force -ErrorAction SilentlyContinue
  }
}
}
