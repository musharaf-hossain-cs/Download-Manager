[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [ValidateSet('Keep', 'Remove')]
    [string] $UserData,

    [string] $RegistrationRoot = 'Registry::HKEY_CURRENT_USER\Software'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if (-not $env:LOCALAPPDATA) { throw 'LOCALAPPDATA is required for a current-user uninstall.' }
if (-not $env:APPDATA) { throw 'APPDATA is required for a current-user uninstall.' }

$localAppData = [IO.Path]::GetFullPath($env:LOCALAPPDATA).TrimEnd('\', '/')
$roamingAppData = [IO.Path]::GetFullPath($env:APPDATA).TrimEnd('\', '/')
$installRoot = Join-Path $localAppData 'Programs\Download Manager'
$dataRoot = Join-Path $localAppData 'DownloadManager'
$manifestRoot = Join-Path $dataRoot 'NativeMessagingHosts'
$stateRoot = Join-Path $dataRoot 'State'
$partialsRoot = Join-Path $stateRoot 'partials'
$onboardingPath = Join-Path $installRoot 'browser-onboarding.json'
$registrationScript = Join-Path $installRoot 'native-host-registration.ps1'
$shortcutPath = Join-Path $roamingAppData 'Microsoft\Windows\Start Menu\Programs\Download Manager.lnk'

function Assert-ProfileChild([string] $Root, [string] $Path, [string] $Label) {
    $prefix = [IO.Path]::GetFullPath($Root).TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
    $target = [IO.Path]::GetFullPath($Path)
    if (-not $target.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw "$Label escaped its current-user profile root: '$target'."
    }
}

function Assert-NotReparsePoint([string] $Path, [string] $Label) {
    if (Test-Path -LiteralPath $Path) {
        $item = Get-Item -LiteralPath $Path -Force
        if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Refusing to recursively remove a reparse-point ${Label}: '$Path'."
        }
    }
}

function Remove-DirectoryBounded([string] $Path) {
    for ($attempt = 1; $attempt -le 5; $attempt++) {
        try {
            Remove-Item -LiteralPath $Path -Recurse -Force
            return
        } catch {
            if ($attempt -eq 5) { throw }
            Start-Sleep -Milliseconds 100
        }
    }
}

function Remove-EmptyDirectory([string] $Path) {
    if ((Test-Path -LiteralPath $Path -PathType Container) -and
        @(Get-ChildItem -LiteralPath $Path -Force).Count -eq 0) {
        Remove-Item -LiteralPath $Path -Force
    }
}

Assert-ProfileChild $localAppData $installRoot 'Install root'
Assert-ProfileChild $localAppData $dataRoot 'Application data root'
Assert-ProfileChild $dataRoot $stateRoot 'Application state root'
Assert-ProfileChild $stateRoot $partialsRoot 'Partial data root'
Assert-ProfileChild $roamingAppData $shortcutPath 'Start Menu shortcut'
Assert-NotReparsePoint $installRoot 'install root'
if ($UserData -eq 'Remove') {
    Assert-NotReparsePoint $dataRoot 'application data root'
    Assert-NotReparsePoint $stateRoot 'application state root'
    Assert-NotReparsePoint $partialsRoot 'partial data root'
}

if (-not (Test-Path -LiteralPath $registrationScript -PathType Leaf)) {
    throw 'The installed native-host registration helper is missing.'
}

# Browser registrations are removed before the executable so no browser can
# retain a launch path to an absent native host if a later cleanup step fails.
& $registrationScript -Action Unregister -ManifestRoot $manifestRoot `
    -OnboardingPath $onboardingPath -RegistrationRoot $RegistrationRoot

if (Test-Path -LiteralPath $shortcutPath) {
    Remove-Item -LiteralPath $shortcutPath -Force
}
if (Test-Path -LiteralPath $installRoot) {
    Remove-DirectoryBounded $installRoot
}
if ($UserData -eq 'Remove') {
    foreach ($name in @('settings.json', 'history.sqlite', 'history.sqlite-wal', 'history.sqlite-shm')) {
        $path = Join-Path $stateRoot $name
        if (Test-Path -LiteralPath $path -PathType Leaf) {
            Remove-Item -LiteralPath $path -Force
        }
    }
    if (Test-Path -LiteralPath $partialsRoot) {
        Remove-DirectoryBounded $partialsRoot
    }
    Remove-EmptyDirectory $stateRoot
    Remove-EmptyDirectory $manifestRoot
    Remove-EmptyDirectory $dataRoot
}
