using System.Windows;
using System.Windows.Controls;
using WpfMessageBox = System.Windows.MessageBox;

namespace WebDavSync;

public partial class RemoteFolderDialog : Window
{
    private readonly AppConfig _config;
    private string _currentPath = "/";

    public string? SelectedFolder { get; private set; }

    public RemoteFolderDialog(AppConfig config)
    {
        _config = config;
        InitializeComponent();
        Loaded += async (_, _) => await LoadFolderAsync(_currentPath);
    }

    private async Task LoadFolderAsync(string path)
    {
        StatusText.Text      = $"Loading {path}…";
        FolderList.IsEnabled = false;

        try
        {
            using var client = new WebDavClient(_config);
            var folders = await client.ListFolderAsync(path);

            FolderList.Items.Clear();
            if (path != "/")
                FolderList.Items.Add("..");

            foreach (var folder in folders.OrderBy(f => f))
            {
                if (folder.TrimEnd('/') == path.TrimEnd('/')) continue;
                FolderList.Items.Add(folder);
            }

            _currentPath    = path;
            StatusText.Text = $"{folders.Count} folder(s) found";
        }
        catch (Exception ex)
        {
            StatusText.Text = $"Error: {ex.Message}";
        }
        finally
        {
            FolderList.IsEnabled = true;
        }
    }

    private void FolderList_DoubleClick(object sender, System.Windows.Input.MouseButtonEventArgs e)
    {
        if (FolderList.SelectedItem is string selected)
        {
            if (selected == "..")
            {
                var parent    = _currentPath.TrimEnd('/');
                var lastSlash = parent.LastIndexOf('/');
                var parentPath = lastSlash >= 0 ? parent[..lastSlash] : "/";
                _ = LoadFolderAsync(parentPath.Length == 0 ? "/" : parentPath);
            }
            else
            {
                _ = LoadFolderAsync(selected.TrimEnd('/'));
            }
        }
    }

    private async void CreateFolder_Click(object sender, RoutedEventArgs e)
    {
        var name = NewFolderBox.Text.Trim();
        if (string.IsNullOrWhiteSpace(name))
        {
            WpfMessageBox.Show("Enter a folder name.", "WebDavSync",
                MessageBoxButton.OK, MessageBoxImage.Warning);
            return;
        }

        var newPath = _currentPath.TrimEnd('/') + "/" + name;
        try
        {
            using var client = new WebDavClient(_config);
            await client.MakeDirectoryAsync(newPath);
            NewFolderBox.Text = string.Empty;
            await LoadFolderAsync(_currentPath);
        }
        catch (Exception ex)
        {
            WpfMessageBox.Show($"Failed to create folder: {ex.Message}", "WebDavSync",
                MessageBoxButton.OK, MessageBoxImage.Error);
        }
    }

    private void Ok_Click(object sender, RoutedEventArgs e)
    {
        SelectedFolder = FolderList.SelectedItem as string ?? _currentPath;
        DialogResult   = true;
        Close();
    }

    private void Cancel_Click(object sender, RoutedEventArgs e)
    {
        DialogResult = false;
        Close();
    }
}
