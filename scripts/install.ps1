<#
.SYNOPSIS
Installs or upgrades the OpenProject CLI and Agent Skill.

.PARAMETER Version
Release version to install, with or without a leading "v". Defaults to the latest GitHub release.

.PARAMETER Destination
Installation directory. Defaults to OPENPROJECT_INSTALL_DIR when set, then the user's local application data directory.

.PARAMETER SkillDestination
Agent Skills directory. Defaults to OPENPROJECT_SKILL_DIR when set, then ~/.agents/skills.

.NOTES
Set OPENPROJECT_NO_MODIFY_PATH=1 to leave the user PATH unchanged.
Set OPENPROJECT_NO_AUTH_PROMPT=1 to skip interactive authentication setup.

.EXAMPLE
./install.ps1

.EXAMPLE
./install.ps1 -Version 0.1.2 -Destination C:\Tools -SkillDestination C:\AgentSkills
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
  ),

  [Parameter(Position = 2)]
  [ValidateNotNullOrEmpty()]
  [string]$SkillDestination = $(
    if ($env:OPENPROJECT_SKILL_DIR) {
      $env:OPENPROJECT_SKILL_DIR
    } else {
      Join-Path ([Environment]::GetFolderPath("UserProfile")) ".agents\skills"
    }
  )
)

$SkillDestinationSupplied = $PSBoundParameters.ContainsKey("SkillDestination")

& {
$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Write-Step {
  param([int]$Number, [string]$Message)
  Write-Host "[$Number/5] $Message"
}

function Confirm-Checksum {
  param([string]$Asset)
  $ChecksumPattern = "^[a-fA-F0-9]{64}\s+\*?$([regex]::Escape($Asset))$"
  $ChecksumLine = Get-Content (Join-Path $Temporary $Checksums) |
    Where-Object { $_ -match $ChecksumPattern } |
    Select-Object -First 1
  if (-not $ChecksumLine) {
    throw "No checksum was published for $Asset."
  }
  $Expected = ($ChecksumLine -split "\s+", 2)[0]
  $Actual = (Get-FileHash (Join-Path $Temporary $Asset) -Algorithm SHA256).Hash
  if ($Actual -ne $Expected) {
    throw "Checksum verification failed for $Asset. The downloaded file may be damaged or unsafe."
  }
}

function Get-InstalledVersion {
  if (-not (Test-Path -LiteralPath $Executable -PathType Leaf)) {
    return $null
  }
  try {
    $ReportedVersion = (& $Executable --version 2>$null | Select-Object -First 1).Trim()
    if ($ReportedVersion -match '^openproject\s+(.+)$') {
      return $Matches[1].Trim()
    }
  } catch {
    return $null
  }
  return $null
}

function Install-AgentSkill {
  param([string]$Root)
  $SkillDirectory = Join-Path $Root "openproject"
  $SkillFile = Join-Path $SkillDirectory "SKILL.md"
  $SkillAction = if (Test-Path -LiteralPath $SkillFile) { "Upgraded" } else { "Installed" }
  try {
    New-Item -ItemType Directory -Force -Path $SkillDirectory | Out-Null
  } catch {
    throw "Could not create $SkillDirectory. Choose a writable directory with -SkillDestination or OPENPROJECT_SKILL_DIR. $($_.Exception.Message)"
  }

  $SkillStaged = Join-Path $SkillDirectory ".SKILL.md.new.$PID"
  try {
    Copy-Item -LiteralPath (Join-Path $Temporary $SkillAsset) -Destination $SkillStaged -Force
    Move-Item -LiteralPath $SkillStaged -Destination $SkillFile -Force
  } finally {
    if (Test-Path -LiteralPath $SkillStaged) {
      Remove-Item -LiteralPath $SkillStaged -Force -ErrorAction SilentlyContinue
    }
  }
  Write-Host "  $SkillAction $SkillFile"
}

function Add-OpenProjectToPath {
  $NormalizedDestination = $Destination.TrimEnd([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar)
  $ProcessEntries = @($env:PATH -split [IO.Path]::PathSeparator | Where-Object { $_ } | ForEach-Object { $_.TrimEnd([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar) })
  $PathIsActive = $ProcessEntries -contains $NormalizedDestination
  if ($env:OPENPROJECT_NO_MODIFY_PATH -eq "1") {
    if (-not $PathIsActive) {
      Write-Host ""
      Write-Host "Note: $Destination is not on PATH. Add it to your user PATH, then open a new terminal."
    }
    return
  }

  try {
    $UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $MachinePath = [Environment]::GetEnvironmentVariable("Path", "Machine")
    $PersistentEntries = @(
      @($UserPath, $MachinePath) |
        ForEach-Object { $_ -split [IO.Path]::PathSeparator } |
        Where-Object { $_ } |
        ForEach-Object { $_.TrimEnd([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar) }
    )
    $PathIsPersistent = $PersistentEntries -contains $NormalizedDestination

    if (-not $PathIsPersistent) {
      $UpdatedUserPath = if ($UserPath) { "$UserPath$([IO.Path]::PathSeparator)$Destination" } else { $Destination }
      [Environment]::SetEnvironmentVariable("Path", $UpdatedUserPath, "User")
      Write-Host ""
      Write-Host "Added $Destination to the user PATH."
    } elseif (-not $PathIsActive) {
      Write-Host ""
      Write-Host "$Destination is already configured in the persistent PATH."
    }

    if (-not $PathIsActive) {
      $env:PATH = "$Destination$([IO.Path]::PathSeparator)$env:PATH"
    }
    if (-not $PathIsPersistent -or -not $PathIsActive) {
      Write-Host "It is available in this PowerShell session and future terminals."
    }
  } catch {
    Write-Host ""
    Write-Warning "Could not update the user PATH automatically. Add $Destination to your user PATH, then open a new terminal. $($_.Exception.Message)"
  }
}

function Write-AuthHandoff {
  Write-Host ""
  Write-Host "ACTION REQUIRED: Finish OpenProject setup in an interactive terminal:"
  Write-Host "  & '$Executable' auth login"
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
$SkillAsset = "openproject-agent-skill.md"
$Checksums = "SHA256SUMS"
$Executable = Join-Path $Destination "openproject.exe"
$Action = if (Test-Path -LiteralPath $Executable) { "Upgraded" } else { "Installed" }
$CliCurrent = $false
$Temporary = $null
$Staged = $null

if (-not $env:OPENPROJECT_SKILL_DIR -and -not $SkillDestinationSupplied) {
  $ClaudeRoot = Join-Path ([Environment]::GetFolderPath("UserProfile")) ".claude"
  $ClaudeSkillDestination = if ((Get-Command claude -ErrorAction SilentlyContinue) -or (Test-Path -LiteralPath $ClaudeRoot)) {
    Join-Path $ClaudeRoot "skills"
  } else {
    $null
  }
} else {
  $ClaudeSkillDestination = $null
}

$SkillDestination = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($SkillDestination)

Write-Host "OpenProject CLI and Agent Skill installer"
Write-Host ""
Write-Host "  Version:     $Version"
Write-Host "  Target:      $Target"
Write-Host "  Destination: $Executable"
Write-Host "  Agent Skill: $(Join-Path (Join-Path $SkillDestination 'openproject') 'SKILL.md')"
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

  if ($Version -eq "latest" -and -not $env:OPENPROJECT_GITLAB_PROJECT) {
    try {
      $LatestRelease = Invoke-WebRequest -Method Head "https://github.com/$Repository/releases/latest"
      $Version = $LatestRelease.BaseResponse.ResponseUri.Segments[-1].Trim("/").TrimStart("v")
    } catch {
      throw "Could not check the latest release. Check your network connection. $($_.Exception.Message)"
    }
    if (-not $Version) {
      throw "Could not determine the latest release version."
    }
  }

  if ((Get-InstalledVersion) -eq $Version) {
    Write-Host "OpenProject $Version is already installed; no upgrade needed."
    $CliCurrent = $true
  }

  $Temporary = Join-Path ([IO.Path]::GetTempPath()) ("openproject-" + [guid]::NewGuid())
  New-Item -ItemType Directory -Force -Path $Temporary | Out-Null

  Write-Step 2 "Downloading release assets"
  if ($env:OPENPROJECT_GITLAB_PROJECT) {
    $GlabArguments = @("release", "download", $RequestedVersion)
    if ($env:OPENPROJECT_GITLAB_HOST) {
      $GlabArguments += @("--hostname", $env:OPENPROJECT_GITLAB_HOST)
    }
    $GlabArguments += @("--repo", $env:OPENPROJECT_GITLAB_PROJECT)
    if (-not $CliCurrent) {
      $GlabArguments += @("--pattern", $Archive)
    }
    $GlabArguments += @("--pattern", $SkillAsset, "--pattern", $Checksums, "--dir", $Temporary)
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
    if (-not $CliCurrent) {
      try {
        Invoke-WebRequest "$Base/$Archive" -OutFile (Join-Path $Temporary $Archive)
      } catch {
        throw "Could not download $Archive. Check the release version and your network connection. $($_.Exception.Message)"
      }
    }
    try {
      Invoke-WebRequest "$Base/$SkillAsset" -OutFile (Join-Path $Temporary $SkillAsset)
    } catch {
      throw "Could not download $SkillAsset. The release may be incomplete. $($_.Exception.Message)"
    }
    try {
      Invoke-WebRequest "$Base/$Checksums" -OutFile (Join-Path $Temporary $Checksums)
    } catch {
      throw "Could not download $Checksums. The release may be incomplete. $($_.Exception.Message)"
    }
  }

  Write-Step 3 "Verifying SHA-256 checksums"
  if (-not $CliCurrent) {
    Confirm-Checksum $Archive
  }
  Confirm-Checksum $SkillAsset

  if ($CliCurrent) {
    Write-Step 4 "Keeping the current OpenProject CLI"
  } else {
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
  }

  Write-Step 5 "Installing OpenProject Agent Skill"
  Install-AgentSkill $SkillDestination
  if ($ClaudeSkillDestination -and $ClaudeSkillDestination -ne $SkillDestination) {
    Install-AgentSkill $ClaudeSkillDestination
  }

  Write-Host ""
  if ($CliCurrent) {
    Write-Host "Success: OpenProject $Version is current and the Agent Skill was refreshed"
  } else {
    Write-Host "Success: $Action $Executable and installed the OpenProject Agent Skill"
  }
  Add-OpenProjectToPath
  Write-Host ""
  Write-Host "Verify the installation:"
  Write-Host "  & '$Executable' --version"
  Write-Host "Restart your agent session if it does not detect the newly installed skill."

  if ($Action -eq "Installed") {
    if ($env:OPENPROJECT_NO_AUTH_PROMPT -eq "1") {
      Write-AuthHandoff
      return
    }

    $CanPrompt = $false
    try {
      $CanPrompt = -not [Console]::IsInputRedirected -and -not [Console]::IsOutputRedirected
    } catch {
      $CanPrompt = $false
    }

    if ($CanPrompt) {
      $ConfigureNow = Read-Host "Configure OpenProject now? [Y/n]"
      if ($ConfigureNow -notmatch '^(n|no)$') {
        & $Executable auth login
        if ($LASTEXITCODE -ne 0) {
          Write-Warning "OpenProject CLI was installed, but setup did not finish."
          Write-AuthHandoff
        }
      } else {
        Write-AuthHandoff
      }
    } else {
      Write-AuthHandoff
    }
  } else {
    Write-Host ""
    Write-Host "Check authentication in this environment:"
    Write-Host "  & '$Executable' auth status --json"
    Write-Host "If it is not authenticated, run:"
    Write-Host "  & '$Executable' auth login"
  }
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
