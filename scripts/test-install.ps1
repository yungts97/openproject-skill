#Requires -Version 5.1
# Offline installer smoke checks; run with both powershell.exe and pwsh.
$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$Installer = Join-Path $PSScriptRoot "install.ps1"
$TestRoot = Join-Path ([IO.Path]::GetTempPath()) ("openproject-installer-test-" + [guid]::NewGuid())
$EnvironmentNames = @(
  "PROCESSOR_ARCHITECTURE", "PROCESSOR_ARCHITEW6432", "OPENPROJECT_INSTALL_DIR",
  "OPENPROJECT_SKILL_DIR", "OPENPROJECT_NO_MODIFY_PATH", "OPENPROJECT_NO_AUTH_PROMPT",
  "OPENPROJECT_GITLAB_PROJECT", "OPENPROJECT_RELEASE_REPOSITORY"
)
$SavedEnvironment = @{}
foreach ($Name in $EnvironmentNames) {
  $SavedEnvironment[$Name] = [Environment]::GetEnvironmentVariable($Name, "Process")
}
$SavedSecurityProtocol = [Net.ServicePointManager]::SecurityProtocol
$TestState = @{}

function Assert-True {
  param([bool]$Condition, [string]$Message)
  if (-not $Condition) { throw $Message }
}

# Shadow only the network boundary; hashing, extraction and installation are real.
function Invoke-WebRequest {
  param([string]$Uri, [string]$Method, [string]$OutFile, [switch]$UseBasicParsing)
  Assert-True $UseBasicParsing.IsPresent "Web requests must use basic parsing in PowerShell 5.1."
  if ($PSVersionTable.PSVersion.Major -lt 6) {
    Assert-True (([Net.ServicePointManager]::SecurityProtocol -band [Net.SecurityProtocolType]::Tls12) -ne 0) "TLS 1.2 was not enabled."
  }
  $TestState.Requests.Add($Uri)
  if ($Method -eq "Head") {
    Assert-True ($Uri -eq "https://github.com/test/openproject/releases/latest") "Unexpected release lookup: $Uri"
    $FinalUri = [uri]"https://github.com/test/openproject/releases/tag/v1.2.3"
    if ($TestState.ResponseShape -eq "Desktop") {
      return [pscustomobject]@{ BaseResponse = [pscustomobject]@{ ResponseUri = $FinalUri } }
    }
    return [pscustomobject]@{ BaseResponse = [pscustomobject]@{ RequestMessage = [pscustomobject]@{ RequestUri = $FinalUri } } }
  }
  Assert-True ($Uri.StartsWith("https://github.com/test/openproject/releases/download/v1.2.3/")) "Unexpected asset URL: $Uri"
  $Asset = ([uri]$Uri).Segments[-1]
  Copy-Item -LiteralPath (Join-Path $TestState.Fixtures $Asset) -Destination $OutFile
  $TestState.TemporaryDirectories.Add((Split-Path -Parent $OutFile))
  if ($TestState.CorruptAsset -eq $Asset) {
    Add-Content -LiteralPath $OutFile -Value "corrupted"
  }
}

try {
  New-Item -ItemType Directory -Path $TestRoot | Out-Null
  $Fixtures = Join-Path $TestRoot "fixtures"
  New-Item -ItemType Directory -Path $Fixtures | Out-Null
  $FixtureExecutable = Join-Path $Fixtures "openproject.exe"
  Set-Content -LiteralPath $FixtureExecutable -Value "fixture executable; never executed"
  foreach ($Target in @("x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc")) {
    Compress-Archive -LiteralPath $FixtureExecutable -DestinationPath (Join-Path $Fixtures "openproject-$Target.zip")
  }
  $References = @("project-selection", "daily-briefing", "work-packages", "setup")
  foreach ($Reference in $References) {
    Set-Content -LiteralPath (Join-Path $Fixtures "openproject-agent-$Reference.md") -Value "# $Reference"
  }

  $Cases = @(
    @{ Name = "desktop-latest-x64"; Architecture = "AMD64"; Native = ""; Shape = "Desktop"; Version = "latest"; Piped = $false; References = $true; Corrupt = ""; Error = "" },
    @{ Name = "core-latest-wow64-piped"; Architecture = "x86"; Native = "AMD64"; Shape = "Core"; Version = "latest"; Piped = $true; References = $true; Corrupt = ""; Error = "" },
    @{ Name = "arm64-pinned"; Architecture = "ARM64"; Native = ""; Shape = "Desktop"; Version = "v1.2.3"; Piped = $false; References = $true; Corrupt = ""; Error = "" },
    @{ Name = "arm64-emulated-latest"; Architecture = "x86"; Native = "ARM64"; Shape = "Core"; Version = "latest"; Piped = $false; References = $true; Corrupt = ""; Error = "" },
    @{ Name = "legacy-skill-pinned"; Architecture = "AMD64"; Native = ""; Shape = "Core"; Version = "1.2.3"; Piped = $false; References = $false; Corrupt = ""; Error = "" },
    @{ Name = "checksum-failure"; Architecture = "AMD64"; Native = ""; Shape = "Desktop"; Version = "latest"; Piped = $false; References = $true; Corrupt = "openproject-agent-setup.md"; Error = "Checksum verification failed" },
    @{ Name = "unsupported-x86"; Architecture = "x86"; Native = ""; Shape = "Desktop"; Version = "latest"; Piped = $false; References = $true; Corrupt = ""; Error = "not supported" }
  )

  foreach ($Case in $Cases) {
    $SkillContent = if ($Case.References) { "# Skill`n[Setup](references/setup.md)" } else { "# Self-contained skill" }
    Set-Content -LiteralPath (Join-Path $Fixtures "openproject-agent-skill.md") -Value $SkillContent
    $ChecksumLines = @(Get-ChildItem -LiteralPath $Fixtures -File | Where-Object { $_.Name -ne "SHA256SUMS" } | ForEach-Object {
      "{0}  {1}" -f (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash, $_.Name
    })
    Set-Content -LiteralPath (Join-Path $Fixtures "SHA256SUMS") -Value $ChecksumLines
    $TestState = @{
      Fixtures = $Fixtures
      ResponseShape = $Case.Shape
      CorruptAsset = $Case.Corrupt
      Requests = New-Object 'System.Collections.Generic.List[string]'
      TemporaryDirectories = New-Object 'System.Collections.Generic.List[string]'
    }
    $CaseRoot = Join-Path $TestRoot $Case.Name
    $env:PROCESSOR_ARCHITECTURE = $Case.Architecture
    $env:PROCESSOR_ARCHITEW6432 = $Case.Native
    $env:OPENPROJECT_INSTALL_DIR = Join-Path $CaseRoot "bin with spaces"
    $env:OPENPROJECT_SKILL_DIR = Join-Path $CaseRoot "skills with spaces"
    $env:OPENPROJECT_NO_MODIFY_PATH = "1"
    $env:OPENPROJECT_NO_AUTH_PROMPT = "1"
    $env:OPENPROJECT_GITLAB_PROJECT = ""
    $env:OPENPROJECT_RELEASE_REPOSITORY = "test/openproject"
    $Failure = $null
    try {
      if ($Case.Piped) {
        Get-Content -LiteralPath $Installer -Raw | Invoke-Expression
      } else {
        & $Installer -Version $Case.Version -Destination $env:OPENPROJECT_INSTALL_DIR -SkillDestination $env:OPENPROJECT_SKILL_DIR
      }
    } catch {
      $Failure = $_.Exception.Message
    }

    Assert-True ([Net.ServicePointManager]::SecurityProtocol -eq $SavedSecurityProtocol) "TLS settings leaked into the caller."
    $InstalledExecutable = Join-Path $env:OPENPROJECT_INSTALL_DIR "openproject.exe"
    if ($Case.Error) {
      Assert-True ($null -ne $Failure -and $Failure.Contains($Case.Error)) "Expected '$($Case.Error)', got '$Failure'."
      Assert-True (-not (Test-Path -LiteralPath $InstalledExecutable)) "Installed an executable after a failed check."
      Assert-True (-not (Test-Path -LiteralPath $env:OPENPROJECT_SKILL_DIR)) "Installed skill files after a failed check."
    } else {
      Assert-True ($null -eq $Failure) "Installation failed: $Failure"
      Assert-True ((Get-FileHash -LiteralPath $InstalledExecutable).Hash -eq (Get-FileHash -LiteralPath $FixtureExecutable).Hash) "Installed executable differs from the verified archive."
      $InstalledSkill = Join-Path $env:OPENPROJECT_SKILL_DIR "openproject"
      Assert-True ((Get-Content -LiteralPath (Join-Path $InstalledSkill "SKILL.md") -Raw).Trim() -eq $SkillContent) "Skill entrypoint was not installed correctly."
      if ($Case.References) {
        foreach ($Reference in $References) {
          Assert-True (Test-Path -LiteralPath (Join-Path $InstalledSkill "references/$Reference.md")) "Missing skill reference $Reference."
        }
      }
      $ExpectedTarget = if ($Case.Architecture -eq "ARM64" -or $Case.Native -eq "ARM64") { "aarch64" } else { "x86_64" }
      Assert-True (@($TestState.Requests | Where-Object { $_.EndsWith("openproject-$ExpectedTarget-pc-windows-msvc.zip") }).Count -eq 1) "Wrong architecture archive requested."
      $ExpectedHeads = if ($Case.Version -eq "latest") { 1 } else { 0 }
      Assert-True (@($TestState.Requests | Where-Object { $_.EndsWith("/latest") }).Count -eq $ExpectedHeads) "Incorrect latest-release lookup count."
    }
    foreach ($Directory in $TestState.TemporaryDirectories) {
      Assert-True (-not (Test-Path -LiteralPath $Directory)) "Installer left its temporary downloads behind."
    }
    Write-Host "PASS: $($Case.Name)"
  }
} finally {
  foreach ($Name in $EnvironmentNames) {
    [Environment]::SetEnvironmentVariable($Name, $SavedEnvironment[$Name], "Process")
  }
  [Net.ServicePointManager]::SecurityProtocol = $SavedSecurityProtocol
  if (Test-Path -LiteralPath $TestRoot) {
    Remove-Item -LiteralPath $TestRoot -Recurse -Force
  }
}
