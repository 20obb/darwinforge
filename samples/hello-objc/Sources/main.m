// Objective-C, UIKit, no storyboard: the window and label are built in code.
// This is the sample project; `darwinforge init Hello` generates the same file.
#import <UIKit/UIKit.h>

@interface AppDelegate : UIResponder <UIApplicationDelegate>
@property (nonatomic, strong) UIWindow *window;
@end

@implementation AppDelegate

- (BOOL)application:(UIApplication *)application
    didFinishLaunchingWithOptions:(NSDictionary *)launchOptions {
    self.window = [[UIWindow alloc] initWithFrame:UIScreen.mainScreen.bounds];

    UIViewController *root = [[UIViewController alloc] init];
    root.view.backgroundColor = UIColor.systemBackgroundColor;

    // Plain resources are copied into the bundle root, so they are reachable
    // by name at runtime.
    NSString *greeting = [NSString stringWithFormat:@"Hello from %@",
        [NSBundle.mainBundle pathForResource:@"greeting" ofType:@"json"] ?: @"darwinforge"];

    UILabel *label = [[UILabel alloc] init];
    label.text = greeting;
    label.numberOfLines = 0;
    label.font = [UIFont preferredFontForTextStyle:UIFontTextStyleTitle2];
    label.textAlignment = NSTextAlignmentCenter;
    label.translatesAutoresizingMaskIntoConstraints = NO;
    [root.view addSubview:label];

    [NSLayoutConstraint activateConstraints:@[
        [label.centerXAnchor constraintEqualToAnchor:root.view.centerXAnchor],
        [label.centerYAnchor constraintEqualToAnchor:root.view.centerYAnchor],
        [label.widthAnchor constraintLessThanOrEqualToAnchor:root.view.widthAnchor constant:-48],
    ]];

    self.window.rootViewController = root;
    [self.window makeKeyAndVisible];
    return YES;
}

@end

int main(int argc, char *argv[]) {
    @autoreleasepool {
        return UIApplicationMain(argc, argv, nil,
                                 NSStringFromClass([AppDelegate class]));
    }
}