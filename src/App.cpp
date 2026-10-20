#include "pch.h"
#include "Core.h"
#include "UI.h"
#include <MddBootstrap.h>
#include <microsoft.ui.xaml.window.h>
#include <winrt/Microsoft.UI.Xaml.Media.Imaging.h>
#include <winrt/Windows.ApplicationModel.DataTransfer.h>
#include <cmath>
#include <algorithm>
#include <ctime>
#include <cwctype>
#include <dwmapi.h>
#include <shobjidl.h>
using namespace winrt;
using namespace Microsoft::UI::Xaml;
using namespace Microsoft::UI::Xaml::Controls;
using namespace Microsoft::UI::Xaml::Media;
using namespace connecty;
namespace
{
int uiCheckResult = 1;
bool UiCheck() { return wcsstr(GetCommandLineW(), L"--ui-check") != nullptr; }
// Reserve the modal slot before asynchronous preparation as well as ShowAsync.
struct DialogScope
{
    bool &busy;
    bool acquired;
    explicit DialogScope(bool &value) : busy(value), acquired(!value) { if (acquired) busy = true; }
    ~DialogScope() { Release(); }
    explicit operator bool() const { return acquired; }
    void Release() { if (acquired) { busy = false; acquired = false; } }
};
bool Active(State s)
{
    return s == State::Connecting || s == State::Forwarding || s == State::Verified || s == State::Retrying ||
           s == State::Suspended;
}
Brush StateBrush(State s)
{
    auto key = s == State::Error ? L"SystemFillColorCriticalBrush"
               : s == State::Retrying || s == State::Connecting || s == State::Suspended
                   ? L"SystemFillColorCautionBrush"
               : s == State::Forwarding || s == State::Verified ? L"AccentTextFillColorPrimaryBrush"
                                                                : L"TextFillColorSecondaryBrush";
    return Application::Current().Resources().Lookup(box_value(key)).as<Brush>();
}
hstring StateGlyph(State s)
{
    if (s == State::Verified)
        return L"\xE774";
    if (s == State::Forwarding)
        return L"\xE774";
    if (s == State::Connecting || s == State::Retrying)
        return L"\xE72C";
    if (s == State::Error)
        return L"\xE783";
    if (s == State::Suspended)
        return L"\xE708";
    return L"\xE7E8";
}
FontIcon AccentIcon(wchar_t const *glyph, double size = 18)
{
    auto icon =
        Markup::XamlReader::Load(
            LR"(<FontIcon xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation" Foreground="{ThemeResource AccentTextFillColorPrimaryBrush}"/>)")
            .as<FontIcon>();
    icon.Glyph(glyph);
    icon.FontSize(size);
    return icon;
}
std::wstring ErrorMessage()
{
    try
    {
        throw;
    }
    catch (hresult_error const &e)
    {
        return e.message().c_str();
    }
    catch (std::exception const &e)
    {
        return Decode(e.what());
    }
    catch (...)
    {
        return L"Unexpected error";
    }
}
std::wstring Clock(uint64_t value)
{
    if (!value)
        return {};
    std::time_t time = std::time_t(value);
    std::tm local{};
    localtime_s(&local, &time);
    wchar_t text[16]{};
    wcsftime(text, 16, L"%H:%M", &local);
    return text;
}
std::wstring LogTime(uint64_t value)
{
    std::time_t time = std::time_t(value);
    std::tm local{};
    localtime_s(&local, &time);
    wchar_t text[32]{};
    wcsftime(text, 32, L"%Y-%m-%d %H:%M:%S", &local);
    return text;
}
Windows::Foundation::IAsyncOperation<Windows::Data::Json::JsonObject>
RequestAsync(std::wstring op, std::wstring id = L"", Windows::Data::Json::JsonObject extra = {})
{
    co_await resume_background();
    co_return AgentClient::Instance().Command(op, id, extra);
}
std::filesystem::path Folder()
{
    wchar_t path[32768]{};
    GetModuleFileNameW(nullptr, path, 32768);
    return std::filesystem::path(path).parent_path();
}
void Logo(Image const &image)
{
    image.Source(Imaging::BitmapImage(
        Windows::Foundation::Uri(L"file:///" + (Folder() / L"Connecty.png").generic_wstring())));
}
struct App : ApplicationT<App, Markup::IXamlMetadataProvider>
{
    Microsoft::UI::Xaml::XamlTypeInfo::XamlControlsXamlMetaDataProvider metadata{nullptr};
    Markup::IXamlType GetXamlType(Windows::UI::Xaml::Interop::TypeName const &type)
    {
        if (!metadata)
            metadata = Microsoft::UI::Xaml::XamlTypeInfo::XamlControlsXamlMetaDataProvider();
        return metadata.GetXamlType(type);
    }
    Markup::IXamlType GetXamlType(hstring const &name)
    {
        if (!metadata)
            metadata = Microsoft::UI::Xaml::XamlTypeInfo::XamlControlsXamlMetaDataProvider();
        return metadata.GetXamlType(name);
    }
    com_array<Markup::XmlnsDefinition> GetXmlnsDefinitions()
    {
        if (!metadata)
            metadata = Microsoft::UI::Xaml::XamlTypeInfo::XamlControlsXamlMetaDataProvider();
        return metadata.GetXmlnsDefinitions();
    }
    Window window{nullptr}, popup{nullptr};
    Grid root{nullptr};
    FrameworkElement popupRoot{nullptr};
    HWND hwnd{}, popupHwnd{};
    HHOOK popupMouseHook{};
    inline static App *popupHookOwner = nullptr;
    Microsoft::UI::Dispatching::DispatcherQueueTimer clipboardTimer{nullptr};
    bool quitting = false, rebuilding = false, mainVisible = false, popupOnly = false, railOpen = true,
         mainReady = false, dialogBusy = false;
    int selected = -1;
    std::vector<Profile> profiles;
    std::vector<TextBlock> rowStates;
    std::vector<StackPanel> rowLabels;
    std::wstring lastLogs, popupSignature;
    template <class T> T Get(wchar_t const *name)
    {
        return root.FindName(name).as<T>();
    }
    template <class F> void Guard(F action)
    {
        try
        {
            action();
        }
        catch (...)
        {
            ShowError(ErrorMessage());
        }
    }
    App()
    {
        UnhandledException([](auto &&, UnhandledExceptionEventArgs const &args) {
            std::ofstream out(DataPath() / L"ui-error.log", std::ios::trunc);
            out << to_string(args.Message()).substr(0, 4096) << '\n';
            args.Handled(true);
            Application::Current().Exit();
        });
    }
    void OnLaunched(LaunchActivatedEventArgs const &)
    {
        try
        {
            Initialize();
        }
        catch (...)
        {
            if (UiCheck())
            {
                std::ofstream out(Folder() / L"ui-check.log", std::ios::trunc);
                out << to_string(ErrorMessage());
                uiCheckResult = 1;
            }
            else MessageBoxW(nullptr, ErrorMessage().c_str(), L"Connecty", MB_ICONERROR);
            Exit();
        }
    }
    void Initialize()
    {
        popupOnly = wcsstr(GetCommandLineW(), L"--tray-popup") != nullptr;
        Resources().MergedDictionaries().Append(XamlControlsResources{});
        if (UiCheck())
        {
            auto main = Markup::XamlReader::Load(MainXaml).as<Grid>();
            auto quick = Markup::XamlReader::Load(PopupXaml).as<FrameworkElement>();
            auto editor = Markup::XamlReader::Load(EditorXaml).as<ScrollViewer>();
            main.FindName(L"ClipboardNotice").as<InfoBar>();
            main.FindName(L"SourceServer").as<FontIcon>();
            quick.FindName(L"PopupPause").as<Button>();
            editor.FindName(L"ValidationMessage").as<InfoBar>();
            uiCheckResult = 0;
            std::ofstream out(Folder() / L"ui-check.log", std::ios::trunc);
            out << "Native XAML startup validation passed\n";
            Exit();
            return;
        }

        window = Window();
        window.Title(L"Connecty");
        window.SystemBackdrop(MicaBackdrop{});
        check_hresult(window.as<IWindowNative>()->get_WindowHandle(&hwnd));
        SetWindowSubclass(hwnd, Subclass, 1, reinterpret_cast<DWORD_PTR>(this));
        AgentClient::Instance().OnChanged([handle = hwnd] { PostMessageW(handle, WM_APP + 10, 0, 0); });
        profiles = LoadProfiles();
        if (popupOnly)
            ShowPopup();
        else
            Show();
    }
    // A cold tray click should not construct the full main-window visual tree.
    void EnsureMain()
    {
        if (mainReady)
            return;
        root = Markup::XamlReader::Load(MainXaml).as<Grid>();
        window.Content(root);
        window.AppWindow().Resize({840, 600});
        window.AppWindow().SetIcon((Folder() / L"Connecty.ico").c_str());
        if (auto presenter =
                window.AppWindow().Presenter().try_as<Microsoft::UI::Windowing::OverlappedPresenter>())
        {
            presenter.PreferredMinimumWidth(740);
            presenter.PreferredMinimumHeight(520);
        }
        window.ExtendsContentIntoTitleBar(true);
        window.SetTitleBar(Get<TitleBar>(L"TitleBar"));
        window.AppWindow().TitleBar().PreferredHeightOption(
            Microsoft::UI::Windowing::TitleBarHeightOption::Tall);
        Get<TitleBar>(L"TitleBar").PaneToggleRequested([this](auto &&, auto &&) { ToggleRail(); });
        Logo(Get<Image>(L"Logo"));
        Get<Button>(L"Add").Click([this](auto &&, auto &&) { Edit(false); });
        Get<Button>(L"FirstAdd").Click([this](auto &&, auto &&) { Edit(false); });
        Get<Button>(L"Edit").Click([this](auto &&, auto &&) { Edit(true); });
        Get<Button>(L"Delete").Click([this](auto &&, auto &&) { Delete(); });
        Get<Button>(L"Connect").Click([this](auto &&, auto &&) {
            if (selected >= 0)
                Connect(profiles[selected].id);
        });
        Get<Button>(L"Reconnect").Click([this](auto &&, auto &&) {
            if (selected >= 0)
                Guard([&] { AgentClient::Instance().Command(L"reconnect", profiles[selected].id); });
        });
        Get<Button>(L"BrowseLogs").Click([this](auto &&, auto &&) { History(); });
        Get<Button>(L"OpenLog").Click([this](auto &&, auto &&) {
            Guard([&] {
                std::wstring text;
                if (selected >= 0)
                    for (auto const &entry : Connection(profiles[selected].id).entries)
                        text += LogTime(entry.time) + L"  " + profiles[selected].name + L"  " + entry.level + L"  " + entry.message + L"\n";
                CopyText(text);
            });
        });
        Get<Button>(L"Settings").Click([this](auto &&, auto &&) { Settings(); });
        root.ActualThemeChanged([this](auto &&, auto &&) {
            Refresh();
            popupSignature.clear();
            if (popupRoot)
                PopupRows();
        });
        Get<ListView>(L"Profiles").SelectionChanged([this](auto &&, auto &&) {
            if (rebuilding)
                return;
            selected = Get<ListView>(L"Profiles").SelectedIndex();
            lastLogs.clear();
            Refresh();
        });
        AnimateSettings(Get<Button>(L"Settings"), Get<AnimatedIcon>(L"SettingsIcon"));
        mainReady = true;
        Rebuild();
    }
    fire_and_forget ShowError(std::wstring text)
    {
        auto lifetime = get_strong();
        DialogScope modal(dialogBusy);
        if (!modal) co_return;
        try
        {
            ContentDialog dialog;
            dialog.XamlRoot(mainVisible ? root.XamlRoot() : popupRoot.XamlRoot());
            dialog.Title(box_value(L"Couldn’t complete that"));
            TextBlock message;
            message.Text(text);
            message.TextWrapping(TextWrapping::Wrap);
            message.MaxWidth(380);
            dialog.Content(message);
            dialog.CloseButtonText(L"Close");
            co_await dialog.ShowAsync();
        }
        catch (...)
        {
            std::ofstream out(DataPath() / L"ui-error.log", std::ios::trunc);
            out << to_string(text).substr(0, 4096) << '\n';
        }
    }
    Snapshot Connection(std::wstring const &id)
    {
        auto view = AgentClient::Instance().Read();
        if (!view.online)
        {
            Snapshot state;
            state.state = State::Error;
            state.detail = L"Agent offline";
            return state;
        }
        auto found = view.connections.find(id);
        if (found != view.connections.end())
            return found->second;
        return {};
    }
    void AnimateSettings(Button const &button, AnimatedIcon const &icon)
    {
        AnimatedIcon::SetState(icon, L"Normal");
        button.PointerEntered([icon](auto &&, auto &&) { AnimatedIcon::SetState(icon, L"PointerOver"); });
        button.PointerExited([icon](auto &&, auto &&) { AnimatedIcon::SetState(icon, L"Normal"); });
        button.GotFocus([icon](auto &&, auto &&) { AnimatedIcon::SetState(icon, L"PointerOver"); });
        button.LostFocus([icon](auto &&, auto &&) { AnimatedIcon::SetState(icon, L"Normal"); });
    }
    void CopyText(std::wstring const &text)
    {
        using namespace Windows::ApplicationModel::DataTransfer;
        DataPackage package;
        package.SetText(text);
        Clipboard::SetContent(package);
        Clipboard::Flush();
        ShowCopied();
    }
    void ShowCopied()
    {
        auto notice = Get<InfoBar>(L"ClipboardNotice");
        notice.IsOpen(true);
        if (!clipboardTimer)
        {
            clipboardTimer = window.DispatcherQueue().CreateTimer();
            clipboardTimer.IsRepeating(false);
            clipboardTimer.Interval(std::chrono::seconds(3));
            clipboardTimer.Tick([weak = get_weak()](auto &&, auto &&) {
                if (auto app = weak.get()) app->Get<InfoBar>(L"ClipboardNotice").IsOpen(false);
            });
        }
        clipboardTimer.Stop();
        clipboardTimer.Start();
    }
    void ToggleRail()
    {
        railOpen = !railOpen;
        Get<ColumnDefinition>(L"RailWidth").Width({railOpen ? 208.0 : 48.0, GridUnitType::Pixel});
        Get<Grid>(L"Rail").Padding(railOpen ? Thickness{10, 12, 10, 12} : Thickness{0, 12, 0, 12});
        Get<Grid>(L"RailHeader").Margin(railOpen ? Thickness{12, 0, 2, 12} : Thickness{0, 0, 0, 12});
        Get<StackPanel>(L"RailFooter").Margin(railOpen ? Thickness{0, 12, 0, 0} : Thickness{0, 12, 0, 0});
        Get<Button>(L"Settings").Padding(railOpen ? Thickness{8, 8, 8, 8} : Thickness{0, 8, 0, 8});
        Get<Button>(L"Settings")
            .HorizontalContentAlignment(railOpen ? HorizontalAlignment::Left : HorizontalAlignment::Center);
        for (auto const &value : Get<ListView>(L"Profiles").Items())
        {
            auto item = value.as<ListViewItem>();
            item.Padding(railOpen ? Thickness{12, 0, 12, 0} : Thickness{0, 0, 0, 0});
            item.HorizontalContentAlignment(railOpen ? HorizontalAlignment::Left
                                                     : HorizontalAlignment::Center);
            item.Content().as<StackPanel>().Padding(railOpen ? Thickness{4, 9, 2, 9} : Thickness{0, 9, 0, 9});
        }
        auto visibility = railOpen ? Visibility::Visible : Visibility::Collapsed;
        Get<TextBlock>(L"RailHeading").Visibility(visibility);
        Get<TextBlock>(L"SettingsText").Visibility(visibility);
        Get<Button>(L"Add").HorizontalAlignment(railOpen ? HorizontalAlignment::Right
                                                         : HorizontalAlignment::Center);
        ToolTipService::SetToolTip(Get<Button>(L"Settings"), box_value(L"Settings"));
        for (auto const &label : rowLabels)
            label.Visibility(visibility);
    }
    void Rebuild()
    {
        rebuilding = true;
        auto list = Get<ListView>(L"Profiles");
        int keep = selected;
        list.Items().Clear();
        rowStates.clear();
        rowLabels.clear();
        for (auto const &p : profiles)
        {
            StackPanel row;
            row.Orientation(Orientation::Horizontal);
            row.Spacing(12);
            row.Padding(railOpen ? Thickness{4, 9, 2, 9} : Thickness{0, 9, 0, 9});
            FontIcon server;
            server.Glyph(L"\xE965");
            server.FontSize(20);
            server.Foreground(StateBrush(State::Verified));
            row.Children().Append(server);
            StackPanel label;
            label.Spacing(3);
            label.Visibility(railOpen ? Visibility::Visible : Visibility::Collapsed);
            rowLabels.push_back(label);
            TextBlock name;
            name.Text(p.name);
            name.FontSize(13);
            name.FontWeight(Windows::UI::Text::FontWeights::SemiBold());
            name.MaxWidth(124);
            name.TextTrimming(TextTrimming::CharacterEllipsis);
            label.Children().Append(name);
            TextBlock state;
            state.FontSize(12);
            state.Opacity(.7);
            label.Children().Append(state);
            rowStates.push_back(state);
            row.Children().Append(label);
            ListViewItem item;
            item.MinWidth(0);
            item.MinHeight(44);
            item.Padding(railOpen ? Thickness{12, 0, 12, 0} : Thickness{0, 0, 0, 0});
            item.Content(row);
            item.HorizontalContentAlignment(railOpen ? HorizontalAlignment::Left
                                                     : HorizontalAlignment::Center);
            Automation::AutomationProperties::SetName(item, p.name);
            ToolTipService::SetToolTip(item, box_value(p.name));
            list.Items().Append(item);
        }
        rebuilding = false;
        if (!profiles.empty())
            list.SelectedIndex(std::clamp(keep, 0, int(profiles.size() - 1)));
        else
            selected = -1;
        Refresh();
    }
    void Sync()
    {
        auto view = AgentClient::Instance().Read();
        bool changed = profiles.size() != view.profiles.size();
        if (!changed)
            for (size_t i = 0; i < profiles.size(); ++i)
                if (profiles[i].id != view.profiles[i].id || profiles[i].name != view.profiles[i].name)
                {
                    changed = true;
                    break;
                }
        profiles = view.profiles;
        if (mainReady)
        {
            if (changed)
                Rebuild();
            else
                Refresh();
        }
        if (popup && IsWindowVisible(popupHwnd))
            PopupRows();
    }
    void Refresh()
    {
        auto view = AgentClient::Instance().Read();
        for (size_t i = 0; i < profiles.size(); ++i)
        {
            auto s = Connection(profiles[i].id);
            if (i < rowStates.size())
            {
                rowStates[i].Text(StateText(s.state));
                auto item = Get<ListView>(L"Profiles").Items().GetAt(uint32_t(i)).as<ListViewItem>();
                Automation::AutomationProperties::SetName(item,
                                                          profiles[i].name + L", " + StateText(s.state));
                ToolTipService::SetToolTip(item, box_value(profiles[i].name + L"\n" + StateText(s.state)));
            }
        }
        Get<Grid>(L"Empty").Visibility(selected < 0 ? Visibility::Visible : Visibility::Collapsed);
        Get<Grid>(L"Details").Visibility(selected < 0 ? Visibility::Collapsed : Visibility::Visible);
        if (selected < 0 || selected >= int(profiles.size()))
            return;
        auto const &p = profiles[selected];
        auto s = Connection(p.id);
        Get<TextBlock>(L"Name").Text(p.name);
        Get<TextBlock>(L"Direction")
            .Text(p.direction == L"local" ? L"Local forwarding" : L"Reverse forwarding");
        Get<TextBlock>(L"Host").Text(p.direction == L"local" ? L"This PC" : p.host);
        Get<TextBlock>(L"TargetHost").Text(p.direction == L"local" ? p.host : L"This PC");
        Get<FrameworkElement>(L"SourceServer").Visibility(p.direction == L"local" ? Visibility::Collapsed : Visibility::Visible);
        Get<FrameworkElement>(L"SourcePc").Visibility(p.direction == L"local" ? Visibility::Visible : Visibility::Collapsed);
        Get<FrameworkElement>(L"TargetServer").Visibility(p.direction == L"local" ? Visibility::Visible : Visibility::Collapsed);
        Get<FrameworkElement>(L"TargetPc").Visibility(p.direction == L"local" ? Visibility::Collapsed : Visibility::Visible);
        Get<FontIcon>(L"DirectionIcon").Glyph(p.direction == L"local" ? L"\xE8A7" : L"\xE8AB");
        Get<TextBlock>(L"RemotePort").Text(p.bindAddress + L":" + std::to_wstring(p.remotePort));
        Get<TextBlock>(L"Target").Text(p.target + L":" + std::to_wstring(p.targetPort));
        ToolTipService::SetToolTip(Get<TextBlock>(L"Host"),
                                   box_value(p.direction == L"local" ? L"This PC" : p.host));
        ToolTipService::SetToolTip(Get<TextBlock>(L"RemotePort"),
                                   box_value(p.bindAddress + L":" + std::to_wstring(p.remotePort)));
        ToolTipService::SetToolTip(Get<TextBlock>(L"Target"),
                                   box_value(p.target + L":" + std::to_wstring(p.targetPort)));
        Get<TextBlock>(L"State").Text(view.online ? StateText(s.state) : L"Agent offline");
        Get<FontIcon>(L"StateIcon").Glyph(StateGlyph(s.state));
        Get<FontIcon>(L"StateIcon").Foreground(StateBrush(s.state));
        // A previous successful probe is not evidence that a retrying session is healthy.
        Get<TextBlock>(L"Age").Text(s.state == State::Verified && s.verifiedAt
                                        ? L"Checked at " + Clock(s.verifiedAt)
                                    : s.state == State::Forwarding ? L"Tunnel ready"
                                    : s.since                      ? L"Since " + Clock(s.since)
                                                                   : L"Ready when you are");
        auto detail = Get<InfoBar>(L"Detail");
        bool showDetail = !s.detail.empty() &&
                          (s.state == State::Error || s.state == State::Retrying || !s.errorCode.empty());
        detail.Message(s.detail);
        detail.Severity(s.state == State::Error ? InfoBarSeverity::Error : InfoBarSeverity::Warning);
        detail.IsOpen(showDetail);
        detail.Visibility(showDetail ? Visibility::Visible : Visibility::Collapsed);
        auto connect = Get<Button>(L"Connect");
        connect.Style(Active(s.state) ? Style{nullptr}
                                      : Resources().Lookup(box_value(L"AccentButtonStyle")).as<Style>());
        Get<TextBlock>(L"ConnectText").Text(Active(s.state) ? L"Disconnect" : L"Connect");
        Get<FontIcon>(L"ConnectIcon").Glyph(Active(s.state) ? L"\xE8CD" : L"\xE71B");
        Automation::AutomationProperties::SetName(connect, Active(s.state) ? L"Disconnect" : L"Connect");
        Get<Button>(L"Reconnect").IsEnabled(Active(s.state) && view.online);
        Get<TextBlock>(L"NoLogs").Visibility(s.entries.empty() ? Visibility::Visible : Visibility::Collapsed);
        Get<Button>(L"OpenLog").IsEnabled(!s.entries.empty());
        if (s.logs != lastLogs)
        {
            lastLogs = s.logs;
            auto list = Get<ListView>(L"Logs");
            list.Items().Clear();
            for (auto it = s.entries.rbegin(); it != s.entries.rend(); ++it)
            {
                Grid row;
                row.Padding({0, 2, 0, 2});
                ColumnDefinition time;
                time.Width(GridLength{68, GridUnitType::Pixel});
                row.ColumnDefinitions().Append(time);
                ColumnDefinition message;
                message.Width(GridLength{1, GridUnitType::Star});
                row.ColumnDefinitions().Append(message);
                TextBlock stamp;
                auto fullTime = LogTime(it->time);
                stamp.Text(fullTime.substr(11));
                ToolTipService::SetToolTip(stamp, box_value(fullTime));
                stamp.FontSize(11);
                stamp.Opacity(.5);
                row.Children().Append(stamp);
                TextBlock text;
                text.Text(it->message);
                text.FontSize(12);
                text.TextWrapping(TextWrapping::Wrap);
                if (it->level == L"error")
                    text.Foreground(StateBrush(State::Error));
                Grid::SetColumn(text, 1);
                row.Children().Append(text);
                list.Items().Append(row);
            }
        }
    }
    fire_and_forget Connect(std::wstring id)
    {
        auto lifetime = get_strong();
        auto state = Connection(id);
        if (Active(state.state))
        {
            Guard([&] { AgentClient::Instance().Command(L"disconnect", id); });
            co_return;
        }
        Windows::Data::Json::JsonObject extra;
        if (state.errorCode == L"key_locked")
        {
            DialogScope modal(dialogBusy);
            if (!modal) co_return;
            ContentDialog dialog;
            dialog.XamlRoot(mainVisible ? root.XamlRoot() : popupRoot.XamlRoot());
            dialog.Title(box_value(L"Unlock private key"));
            PasswordBox password;
            password.Header(box_value(L"Passphrase"));
            dialog.Content(password);
            dialog.PrimaryButtonText(L"Connect");
            dialog.CloseButtonText(L"Cancel");
            if (co_await dialog.ShowAsync() != ContentDialogResult::Primary)
                co_return;
            extra.SetNamedValue(L"passphrase",
                                Windows::Data::Json::JsonValue::CreateStringValue(password.Password()));
            password.Password(L"");
        }
        Guard([&] { AgentClient::Instance().Command(L"connect", id, extra); });
        if (extra.HasKey(L"passphrase"))
            extra.Remove(L"passphrase");
    }
    fire_and_forget Edit(bool existing)
    {
        auto lifetime = get_strong();
        DialogScope modal(dialogBusy);
        if (!modal) co_return;
        int index = existing ? selected : -1;
        if (existing && index < 0)
            co_return;
        Profile p = existing ? profiles[index] : Profile{};
        if (!existing)
            p.id = NewId();
        auto view = Markup::XamlReader::Load(EditorXaml).as<ScrollViewer>();
        auto field = [&]<class T>(wchar_t const *name) { return view.FindName(name).as<T>(); };
        // Hold the control directly: dialog hosting can change the namescope used by FindName.
        auto validation = field.operator()<InfoBar>(L"ValidationMessage");
        check_bool(validation != nullptr);
        field.operator()<TextBox>(L"Name").Text(p.name);
        field.operator()<TextBox>(L"Host").Text(p.host);
        field.operator()<TextBox>(L"Target").Text(p.target);
        field.operator()<TextBox>(L"Probe").Text(p.probe);
        field.operator()<TextBox>(L"Hostname").Text(p.hostname);
        field.operator()<TextBox>(L"User").Text(p.user);
        field.operator()<TextBox>(L"Identity").Text(p.identityFile);
        field.operator()<TextBox>(L"KnownHosts").Text(p.knownHosts);
        field.operator()<NumberBox>(L"Port").Value(p.port);
        field.operator()<NumberBox>(L"RemotePort").Value(p.remotePort);
        field.operator()<NumberBox>(L"TargetPort").Value(p.targetPort);
        field.operator()<ToggleSwitch>(L"Retry").IsOn(p.retry);
        field.operator()<ToggleSwitch>(L"Auto").IsOn(p.autoConnect);
        field.operator()<ComboBox>(L"Direction").SelectedIndex(p.direction == L"local" ? 1 : 0);
        ContentDialog dialog;
        dialog.XamlRoot(root.XamlRoot());
        dialog.Title(box_value(existing ? L"Edit connection" : L"New connection"));
        dialog.Content(view);
        dialog.PrimaryButtonText(L"Save");
        dialog.CloseButtonText(L"Cancel");
        dialog.DefaultButton(ContentDialogButton::Primary);
        dialog.PrimaryButtonClick([&, validation](auto &&, ContentDialogButtonClickEventArgs const &args) {
            args.Cancel(true);
            p.name = field.operator()<TextBox>(L"Name").Text();
            p.host = field.operator()<TextBox>(L"Host").Text();
            p.target = field.operator()<TextBox>(L"Target").Text();
            p.probe = field.operator()<TextBox>(L"Probe").Text();
            p.hostname = field.operator()<TextBox>(L"Hostname").Text();
            p.user = field.operator()<TextBox>(L"User").Text();
            p.identityFile = field.operator()<TextBox>(L"Identity").Text();
            p.knownHosts = field.operator()<TextBox>(L"KnownHosts").Text();
            auto port = [&](wchar_t const *name, bool zero = false) {
                double value = field.operator()<NumberBox>(name).Value();
                return std::isfinite(value) && value == std::floor(value) && value >= (zero ? 0 : 1) &&
                               value <= 65535
                           ? int(value)
                           : -1;
            };
            p.port = port(L"Port", true);
            p.remotePort = port(L"RemotePort");
            p.targetPort = port(L"TargetPort");
            p.retry = field.operator()<ToggleSwitch>(L"Retry").IsOn();
            p.autoConnect = field.operator()<ToggleSwitch>(L"Auto").IsOn();
            p.direction =
                field.operator()<ComboBox>(L"Direction").SelectedIndex() == 1 ? L"local" : L"reverse";
            auto error = Validate(p);
            if (!error.empty())
            {
                validation.Message(error);
                validation.IsOpen(true);
                view.ChangeView(nullptr, 0.0, nullptr);
            }
            else
                args.Cancel(false);
        });
        auto result = co_await dialog.ShowAsync();
        modal.Release();
        if (result != ContentDialogResult::Primary)
            co_return;
        Guard([&] {
            auto next = profiles;
            if (existing)
                next[index] = p;
            else
                next.push_back(p);
            SaveProfiles(next);
            profiles = std::move(next);
            if (!existing)
                selected = int(profiles.size() - 1);
            Rebuild();
        });
    }
    fire_and_forget Delete()
    {
        auto lifetime = get_strong();
        DialogScope modal(dialogBusy);
        if (!modal) co_return;
        int index = selected;
        if (index < 0)
            co_return;
        ContentDialog dialog;
        dialog.XamlRoot(root.XamlRoot());
        dialog.Title(box_value(L"Remove " + profiles[index].name + L"?"));
        dialog.PrimaryButtonText(L"Remove");
        dialog.CloseButtonText(L"Cancel");
        auto result = co_await dialog.ShowAsync();
        modal.Release();
        if (result == ContentDialogResult::Primary)
            Guard([&] {
                auto next = profiles;
                next.erase(next.begin() + index);
                SaveProfiles(next);
                profiles = std::move(next);
                selected = -1;
                Rebuild();
            });
    }
    fire_and_forget History()
    {
        auto lifetime = get_strong();
        DialogScope modal(dialogBusy);
        if (!modal) co_return;
        try
        {
            auto response = co_await RequestAsync(L"history");
            auto data = response.GetNamedObject(L"data");
            auto records = data.GetNamedArray(L"records");
            auto panel = Markup::XamlReader::Load(LR"xaml(
<StackPanel xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation" xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml" Spacing="12" MinWidth="480">
 <TextBox x:Name="Search" PlaceholderText="Search activity" AutomationProperties.Name="Search activity"/>
 <CheckBox x:Name="All" Content="All connections"/>
 <ListView x:Name="Rows" Height="280" SelectionMode="None" Background="Transparent" BorderThickness="0" Padding="0" AutomationProperties.Name="Connection history"/>
 <TextBlock x:Name="Note" Visibility="Collapsed" FontSize="11" TextWrapping="Wrap" Foreground="{ThemeResource TextFillColorSecondaryBrush}"/>
</StackPanel>)xaml").as<StackPanel>();
            auto search = panel.FindName(L"Search").as<TextBox>();
            auto all = panel.FindName(L"All").as<CheckBox>();
            auto rows = panel.FindName(L"Rows").as<ListView>();
            auto note = panel.FindName(L"Note").as<TextBlock>();
            std::wstring id = selected >= 0 ? profiles[selected].id : L"";
            all.IsChecked(box_value(id.empty()).as<Windows::Foundation::IReference<bool>>());
            auto exported = std::make_shared<std::wstring>();
            auto render = [records, id, exported, ws=make_weak(search), wa=make_weak(all), wr=make_weak(rows)] {
                auto search = ws.get(); auto all = wa.get(); auto rows = wr.get();
                if (!search || !all || !rows) return;
                std::wstring query = search.Text().c_str();
                std::transform(query.begin(), query.end(), query.begin(), [](wchar_t c){ return std::towlower(c); });
                rows.Items().Clear(); exported->clear();
                for (int i = int(records.Size()) - 1; i >= 0; --i)
                {
                    auto record = records.GetAt(uint32_t(i)).GetObject();
                    if (!all.IsChecked().Value() && record.GetNamedString(L"profileId") != id) continue;
                    std::wstring profile = record.GetNamedString(L"profile").c_str();
                    std::wstring message = record.GetNamedString(L"message").c_str();
                    std::wstring level = record.GetNamedString(L"level").c_str();
                    std::wstring text = profile + L" " + message + L" " + level + L" " + record.GetNamedString(L"code").c_str();
                    std::transform(text.begin(), text.end(), text.begin(), [](wchar_t c){ return std::towlower(c); });
                    if (text.find(query) == std::wstring::npos) continue;
                    auto stamp = LogTime(uint64_t(record.GetNamedNumber(L"time")));
                    *exported += stamp + L"  " + profile + L"  " + level + L"  " + message + L"\n";
                    StackPanel row; row.Spacing(4); row.Padding({0,6,0,6});
                    TextBlock header; header.Text(stamp + L"    " + profile); header.FontSize(11); header.Opacity(.65);
                    TextBlock body; body.Text(message); body.FontSize(12); body.TextWrapping(TextWrapping::Wrap); body.IsTextSelectionEnabled(true);
                    if (level == L"error") body.Foreground(StateBrush(State::Error));
                    row.Children().Append(header); row.Children().Append(body); rows.Items().Append(row);
                }
                if (!rows.Items().Size()) { TextBlock empty; empty.Text(L"No matching activity."); rows.Items().Append(empty); }
            };
            search.TextChanged([render](auto &&, auto &&){ render(); });
            all.Checked([render](auto &&, auto &&){ render(); });
            all.Unchecked([render](auto &&, auto &&){ render(); });
            render();
            std::wstring info;
            auto error = data.GetNamedValue(L"error", Windows::Data::Json::JsonValue::CreateNullValue());
            if (error.ValueType() == Windows::Data::Json::JsonValueType::String) info += L"\n" + std::wstring(error.GetString());
            if (data.GetNamedNumber(L"dropped", 0) > 0) info += L"\nSome events were dropped.";
            note.Text(info);
            note.Visibility(info.empty() ? Visibility::Collapsed : Visibility::Visible);
            ContentDialog dialog; dialog.XamlRoot(root.XamlRoot()); dialog.Title(box_value(L"Connection history"));
            dialog.Content(panel); dialog.PrimaryButtonText(L"Copy"); dialog.CloseButtonText(L"Close");
            if (co_await dialog.ShowAsync() == ContentDialogResult::Primary)
            {
                CopyText(*exported);
            }
        }
        catch (...) { modal.Release(); ShowError(ErrorMessage()); }
    }
    fire_and_forget Settings()
    {
        auto lifetime = get_strong();
        DialogScope modal(dialogBusy);
        if (!modal) co_return;
        try
        {
            auto response = co_await RequestAsync(L"log_options");
            auto options = response.GetNamedObject(L"data").GetNamedObject(L"settings");
            auto panel = Markup::XamlReader::Load(LR"xaml(
<ScrollViewer xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation" xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml" MaxHeight="350"><StackPanel Spacing="16" MinWidth="380" Padding="0,0,8,0">
 <InfoBar x:Name="LoggingError" IsOpen="False" IsClosable="False" Severity="Error"/><ToggleSwitch x:Name="Startup" Header="Run at sign-in"/>
 <ToggleSwitch x:Name="SaveLogs" Header="Save connection history"/>
 <TextBox x:Name="LogFolder" Header="Log folder"/>
 <Grid ColumnSpacing="12"><Grid.ColumnDefinitions><ColumnDefinition Width="*"/><ColumnDefinition Width="*"/></Grid.ColumnDefinitions><NumberBox x:Name="LogDays" Header="Keep for (days)" Minimum="1" Maximum="30" SpinButtonPlacementMode="Compact"/><NumberBox x:Name="LogSize" Grid.Column="1" Header="File limit (MiB)" Minimum="1" Maximum="16" SpinButtonPlacementMode="Compact"/></Grid>
 <StackPanel Orientation="Horizontal" Spacing="12"><Image x:Name="SettingsLogo" Width="36" Height="36"/><StackPanel VerticalAlignment="Center" Spacing="3"><TextBlock Text="Connecty 0.3.2" FontSize="13" FontWeight="SemiBold"/><TextBlock FontSize="12" Foreground="{ThemeResource TextFillColorSecondaryBrush}"><Run Text="By "/><Hyperlink NavigateUri="https://chromatic.hu">Chromatic</Hyperlink></TextBlock></StackPanel></StackPanel>
</StackPanel></ScrollViewer>)xaml").as<ScrollViewer>();
            Logo(panel.FindName(L"SettingsLogo").as<Image>());
            auto startup = panel.FindName(L"Startup").as<ToggleSwitch>(); startup.IsOn(StartupEnabled());
            auto save = panel.FindName(L"SaveLogs").as<ToggleSwitch>(); save.IsOn(options.GetNamedBoolean(L"enabled"));
            auto folder = panel.FindName(L"LogFolder").as<TextBox>(); folder.Text(options.GetNamedString(L"directory"));
            auto days = panel.FindName(L"LogDays").as<NumberBox>(); days.Value(options.GetNamedNumber(L"keepDays"));
            auto size = panel.FindName(L"LogSize").as<NumberBox>(); size.Value(options.GetNamedNumber(L"maxMib"));
            auto feedback = panel.FindName(L"LoggingError").as<InfoBar>();
            ContentDialog dialog; dialog.XamlRoot(root.XamlRoot()); dialog.Title(box_value(L"Settings")); dialog.Content(panel);
            dialog.PrimaryButtonText(L"Save"); dialog.CloseButtonText(L"Close"); dialog.DefaultButton(ContentDialogButton::Primary);
            dialog.PrimaryButtonClick([this, startup, save, folder, days, size, feedback](ContentDialog, ContentDialogButtonClickEventArgs args) -> fire_and_forget {
                auto deferral = args.GetDeferral();
                args.Cancel(true);
                try
                {
                    using namespace Windows::Data::Json;
                    if (!std::isfinite(days.Value()) || !std::isfinite(size.Value())) throw std::runtime_error("Enter a retention period and file size");
                    JsonObject settings;
                    settings.SetNamedValue(L"enabled", JsonValue::CreateBooleanValue(save.IsOn()));
                    settings.SetNamedValue(L"directory", JsonValue::CreateStringValue(folder.Text()));
                    settings.SetNamedValue(L"keepDays", JsonValue::CreateNumberValue(std::floor(days.Value())));
                    settings.SetNamedValue(L"maxMib", JsonValue::CreateNumberValue(std::floor(size.Value())));
                    JsonObject extra; extra.SetNamedValue(L"settings", settings);
                    co_await RequestAsync(L"logging", L"", extra);
                    if (startup.IsOn() != StartupEnabled()) { JsonObject command; command.SetNamedValue(L"enabled", JsonValue::CreateBooleanValue(startup.IsOn())); co_await RequestAsync(L"startup", L"", command); }
                    args.Cancel(false);
                }
                catch (...) { feedback.Message(ErrorMessage()); feedback.IsOpen(true); }
                deferral.Complete();
            });
            co_await dialog.ShowAsync();
        }
        catch (...) { modal.Release(); ShowError(ErrorMessage()); }
    }
    void Show()
    {
        EnsureMain();
        mainVisible = true;
        popupOnly = false;
        if (popup)
            HidePopup();
        window.AppWindow().Show();
        window.Activate();
        ShowWindow(hwnd, SW_RESTORE);
        SetForegroundWindow(hwnd);
    }
    void PopupRows()
    {
        if (!popupRoot)
            return;
        std::wstring signature;
        for (auto const &p : profiles)
            signature += p.id + L"\n" + p.name + L"\n" + std::to_wstring(int(Connection(p.id).state)) + L"\n";
        // Keep keyboard focus intact when only a probe timestamp or log entry changes.
        if (signature == popupSignature &&
            popupRoot.FindName(L"PopupRows").as<StackPanel>().Children().Size())
            return;
        popupSignature = signature;
        auto rows = popupRoot.FindName(L"PopupRows").as<StackPanel>();
        rows.Children().Clear();
        unsigned count = 0;
        for (auto const &p : profiles)
        {
            auto s = Connection(p.id);
            if (s.state == State::Forwarding || s.state == State::Verified)
                ++count;
            if (rows.Children().Size() >= 5)
                continue;
            auto row = Markup::XamlReader::Load(LR"(
<Border xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation" xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml" Background="{ThemeResource CardBackgroundFillColorDefaultBrush}" CornerRadius="8" Padding="10,8">
 <Grid ColumnSpacing="10"><Grid.ColumnDefinitions><ColumnDefinition Width="24"/><ColumnDefinition Width="*"/><ColumnDefinition Width="36"/></Grid.ColumnDefinitions>
  <FontIcon x:Name="Icon" FontSize="19"/>
  <StackPanel Grid.Column="1" Spacing="3" VerticalAlignment="Center"><TextBlock x:Name="Name" FontSize="13" FontWeight="SemiBold" TextTrimming="CharacterEllipsis"/><TextBlock x:Name="State" FontSize="11" Foreground="{ThemeResource TextFillColorSecondaryBrush}"/></StackPanel>
  <Button x:Name="Toggle" Grid.Column="2" Style="{ThemeResource SubtleButtonStyle}" Width="36" Height="36" Padding="8" VerticalAlignment="Center"><FontIcon x:Name="ActionIcon" Glyph="&#xE71B;" FontSize="16"/></Button>
 </Grid>
</Border>)")
                           .as<Border>();
            auto name = row.FindName(L"Name").as<TextBlock>();
            name.Text(p.name);
            ToolTipService::SetToolTip(name, box_value(p.name));
            auto state = row.FindName(L"State").as<TextBlock>();
            state.Text(StateText(s.state));
            auto statusIcon = row.FindName(L"Icon").as<FontIcon>();
            statusIcon.Glyph(StateGlyph(s.state));
            statusIcon.Foreground(StateBrush(s.state));
            auto toggle = row.FindName(L"Toggle").as<Button>();
            row.FindName(L"ActionIcon").as<FontIcon>().Glyph(Active(s.state) ? L"\xE8CD" : L"\xE71B");
            auto action = (Active(s.state) ? L"Disconnect " : L"Connect ") + p.name;
            Automation::AutomationProperties::SetName(toggle, action);
            ToolTipService::SetToolTip(toggle, box_value(action));
            toggle.Click([this, id = p.id](auto &&, auto &&) { Connect(id); });
            rows.Children().Append(row);
        }
        if (profiles.empty())
        {
            TextBlock empty;
            empty.Text(L"No connections yet");
            empty.FontSize(13);
            empty.Margin({10, 12, 10, 12});
            rows.Children().Append(empty);
        }
        popupRoot.FindName(L"PopupCount")
            .as<TextBlock>()
            .Text(std::to_wstring(count) + L" of " + std::to_wstring(profiles.size()) + L" connected");
        popupRoot.FindName(L"PopupPause")
            .as<Button>()
            .IsEnabled(std::any_of(profiles.begin(), profiles.end(),
                                   [&](auto const &p) { return Active(Connection(p.id).state); }));
    }
    void ShowPopup()
    {
        if (!popup)
        {
            popup = Window();
            popup.Title(L"Connecty quick controls");
            popup.SystemBackdrop(DesktopAcrylicBackdrop{});
            popupRoot = Markup::XamlReader::Load(PopupXaml).as<FrameworkElement>();
            popup.Content(popupRoot);
            check_hresult(popup.as<IWindowNative>()->get_WindowHandle(&popupHwnd));
            SetWindowSubclass(popupHwnd, PopupSubclass, 2, reinterpret_cast<DWORD_PTR>(this));
            auto presenter =
                popup.AppWindow().Presenter().as<Microsoft::UI::Windowing::OverlappedPresenter>();
            presenter.SetBorderAndTitleBar(false, false);
            presenter.IsResizable(false);
            presenter.IsAlwaysOnTop(true);
            presenter.IsMaximizable(false);
            presenter.IsMinimizable(false);
            // Let DWM clip and shadow the acrylic; no second XAML outline or resize frame.
            auto style = GetWindowLongPtrW(popupHwnd, GWL_STYLE);
            SetWindowLongPtrW(popupHwnd, GWL_STYLE, style & ~(WS_BORDER | WS_DLGFRAME | WS_THICKFRAME));
            SetWindowLongPtrW(popupHwnd, GWL_EXSTYLE,
                              (GetWindowLongPtrW(popupHwnd, GWL_EXSTYLE) | WS_EX_TOOLWINDOW) &
                                  ~WS_EX_APPWINDOW);
            SetWindowPos(popupHwnd, nullptr, 0, 0, 0, 0,
                         SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
            DWM_WINDOW_CORNER_PREFERENCE corners = DWMWCP_ROUND;
            DwmSetWindowAttribute(popupHwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &corners, sizeof(corners));
            COLORREF border = DWMWA_COLOR_NONE;
            DwmSetWindowAttribute(popupHwnd, DWMWA_BORDER_COLOR, &border, sizeof(border));
            BOOL dark = TRUE;
            DwmSetWindowAttribute(popupHwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark, sizeof(dark));
            popupRoot.KeyDown([this](auto &&, Input::KeyRoutedEventArgs const &e) {
                if (e.Key() == Windows::System::VirtualKey::Escape)
                {
                    e.Handled(true);
                    HidePopup();
                }
            });
            Logo(popupRoot.FindName(L"PopupLogo").as<Image>());
            AnimateSettings(popupRoot.FindName(L"PopupSettings").as<Button>(),
                            popupRoot.FindName(L"SettingsIcon").as<AnimatedIcon>());
            popupRoot.FindName(L"PopupOpen").as<Button>().Click([this](auto &&, auto &&) { Show(); });
            popupRoot.FindName(L"PopupSettings").as<Button>().Click([this](auto &&, auto &&) {
                Show();
                Settings();
            });
            popupRoot.FindName(L"PopupPause").as<Button>().Click([this](auto &&, auto &&) {
                Guard([&] { AgentClient::Instance().Command(L"disconnect_all"); });
            });
            popupRoot.FindName(L"PopupStop").as<Button>().Click([this](auto &&, auto &&) {
                Guard([&] { AgentClient::Instance().Command(L"shutdown"); });
                Quit();
            });
            popup.Activated([this](auto &&, WindowActivatedEventArgs const &e) {
                if (e.WindowActivationState() == WindowActivationState::Deactivated)
                {
                    HidePopup();
                }
            });
        }
        PopupRows();
        POINT point;
        GetCursorPos(&point);
        MONITORINFO monitor{sizeof(monitor)};
        GetMonitorInfoW(MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST), &monitor);
        RECT work = monitor.rcWork;
        popupRoot.Measure({320, 1000});
        int width = 320, height = int(std::ceil(popupRoot.DesiredSize().Height));
        int dpi = int(GetDpiForWindow(popupHwnd));
        width = MulDiv(width, dpi, 96);
        height = MulDiv(height, dpi, 96);
        popup.AppWindow().MoveAndResize({std::clamp(point.x - width, work.left, work.right - width),
                                         std::clamp(point.y - height, work.top, work.bottom - height), width,
                                         height});
        popup.AppWindow().Show();
        popup.Activate();
        SetForegroundWindow(popupHwnd);
        popupRoot.FindName(L"PopupOpen").as<Button>().Focus(FocusState::Programmatic);
        if (!popupMouseHook)
        {
            popupHookOwner = this;
            popupMouseHook = SetWindowsHookExW(WH_MOUSE_LL, PopupMouse, GetModuleHandleW(nullptr), 0);
        }
    }
    void DisarmPopup()
    {
        if (popupMouseHook) { UnhookWindowsHookEx(popupMouseHook); popupMouseHook = nullptr; }
        if (popupHookOwner == this) popupHookOwner = nullptr;
    }
    void HidePopup()
    {
        DisarmPopup();
        if (popup) popup.AppWindow().Hide();
        if (popupOnly && !mainVisible) Quit();
    }
    static LRESULT CALLBACK PopupMouse(int code, WPARAM message, LPARAM data)
    {
        // Only while the panel is visible. Never consume or record outside input.
        auto app = popupHookOwner;
        if (code == HC_ACTION && app && (message == WM_LBUTTONDOWN || message == WM_RBUTTONDOWN ||
                                        message == WM_MBUTTONDOWN || message == WM_XBUTTONDOWN))
        {
            RECT bounds{};
            auto point = reinterpret_cast<MSLLHOOKSTRUCT const *>(data)->pt;
            if (GetWindowRect(app->popupHwnd, &bounds) && !PtInRect(&bounds, point))
                PostMessageW(app->popupHwnd, WM_APP + 12, 0, 0);
        }
        return CallNextHookEx(nullptr, code, message, data);
    }
    static LRESULT CALLBACK PopupSubclass(HWND h, UINT message, WPARAM w, LPARAM l, UINT_PTR, DWORD_PTR data)
    {
        auto app = reinterpret_cast<App *>(data);
        if (message == WM_APP + 12 || message == WM_CANCELMODE || message == WM_CLOSE)
        {
            app->HidePopup();
            return 0;
        }
        return DefSubclassProc(h, message, w, l);
    }
    void Quit()
    {
        if (quitting)
            return;
        quitting = true;
        DisarmPopup();
        if (popupHwnd) RemoveWindowSubclass(popupHwnd, PopupSubclass, 2);
        if (clipboardTimer) clipboardTimer.Stop();
        AgentClient::Instance().OnChanged({});
        RemoveWindowSubclass(hwnd, Subclass, 1);
        if (popup)
            popup.Close();
        window.Close();
        Exit();
    }
    static LRESULT CALLBACK Subclass(HWND h, UINT m, WPARAM w, LPARAM l, UINT_PTR, DWORD_PTR data)
    {
        auto app = reinterpret_cast<App *>(data);
        if (m == WM_CLOSE)
        {
            app->Quit();
            return 0;
        }
        if (m == WM_APP + 8)
        {
            app->Show();
            return 0;
        }
        if (m == WM_APP + 11)
        {
            app->ShowPopup();
            return 0;
        }
        if (m == WM_APP + 10)
        {
            app->Sync();
            return 0;
        }
        return DefSubclassProc(h, m, w, l);
    }
};
} // namespace
int WINAPI wWinMain(HINSTANCE, HINSTANCE, PWSTR, int)
{
    if (wcsstr(GetCommandLineW(), L"--runtime-check"))
    {
        PACKAGE_VERSION minimum{};
        minimum.Major = 2;
        minimum.Minor = 5;
        minimum.Build = 1;
        HRESULT result =
            MddBootstrapInitialize2(0x00020000, L"", minimum, MddBootstrapInitializeOptions_None);
        if (SUCCEEDED(result))
            MddBootstrapShutdown();
        return SUCCEEDED(result) ? 0 : 1;
    }
    SetCurrentProcessExplicitAppUserModelID(L"Chromatic.Connecty");
    HANDLE mutex = UiCheck() ? nullptr : CreateMutexW(nullptr, FALSE, L"Local\\Connecty.SingleInstance");
    if (!UiCheck() && GetLastError() == ERROR_ALREADY_EXISTS)
    {
        if (HWND existing = FindWindowW(nullptr, L"Connecty"))
            PostMessageW(existing, wcsstr(GetCommandLineW(), L"--tray-popup") ? WM_APP + 11 : WM_APP + 8, 0,
                         0);
        if (mutex)
            CloseHandle(mutex);
        return 0;
    }
    int code = 0;
    try
    {
        PACKAGE_VERSION minimum{};
        minimum.Major = 2;
        minimum.Minor = 5;
        minimum.Build = 1;
        check_hresult(MddBootstrapInitialize2(0x00020000, L"", minimum, MddBootstrapInitializeOptions_None));
        init_apartment(apartment_type::single_threaded);
        Application::Start([](auto &&) { make<App>(); });
        if (UiCheck()) code = uiCheckResult;
        uninit_apartment();
        MddBootstrapShutdown();
    }
    catch (...)
    {
        if (UiCheck())
        {
            std::ofstream out(Folder() / L"ui-check.log", std::ios::trunc);
            out << to_string(ErrorMessage());
        }
        else MessageBoxW(nullptr, ErrorMessage().c_str(), L"Connecty could not start", MB_ICONERROR);
        code = 1;
    }
    if (mutex)
        CloseHandle(mutex);
    return code;
}
